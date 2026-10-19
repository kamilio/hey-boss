import Foundation
import AVFoundation
import AudioToolbox
import CoreAudio
import Accelerate
import TalkCore

struct InputDevice {
    let id: AudioDeviceID
    let uid: String
    let name: String
    var json: [String: Any] { ["uid": uid, "name": name, "default": id == AudioDevices.defaultInput] }
}

enum AudioDevices {
    static var defaultInput: AudioDeviceID {
        var address = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDefaultInputDevice, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var device: AudioDeviceID = 0, size = UInt32(MemoryLayout<AudioDeviceID>.size)
        AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &device)
        return device
    }
    static func list() -> [InputDevice] {
        var address = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDevices, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size) == noErr else { return [] }
        var ids = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
        guard AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &ids) == noErr else { return [] }
        return ids.compactMap { id in
            var streamAddress = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyStreams, mScope: kAudioDevicePropertyScopeInput, mElement: kAudioObjectPropertyElementMain)
            var streamSize: UInt32 = 0
            guard AudioObjectGetPropertyDataSize(id, &streamAddress, 0, nil, &streamSize) == noErr, streamSize > 0,
                  let uid = string(id, kAudioDevicePropertyDeviceUID), let name = string(id, kAudioObjectPropertyName) else { return nil }
            guard !uid.hasPrefix("CADefaultDeviceAggregate-") else { return nil }
            return InputDevice(id: id, uid: uid, name: name)
        }
    }
    private static func string(_ id: AudioDeviceID, _ selector: AudioObjectPropertySelector) -> String? {
        var address = AudioObjectPropertyAddress(mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var value: Unmanaged<CFString>?, size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        guard AudioObjectGetPropertyData(id, &address, 0, nil, &size, &value) == noErr else { return nil }
        return value?.takeUnretainedValue() as String?
    }
}

final class AudioCapture {
    private var engine: AVAudioEngine?
    private let control = DispatchQueue(label: "hey-talk.audio-control", qos: .userInitiated)
    private let processing = DispatchQueue(label: "hey-talk.audio", qos: .userInteractive)
    private let lock = NSLock()
    private var samples: [Float] = []
    private var rms: Double = 0
    private var peak: Float = 0
    private var fault: String?
    private var pending = 0
    private var token: UInt64 = 0
    private var observer: NSObjectProtocol?
    private var routeRestarts = 0
    var onFailure: ((String) -> Void)?
    var level: Double { lock.lock(); defer { lock.unlock() }; return rms }
    var sampleCount: Int { lock.lock(); defer { lock.unlock() }; return samples.count }
    var maximumLevel: Float { lock.lock(); defer { lock.unlock() }; return peak }

    func start(uid: String?, completion: @escaping (Result<Void, Error>) -> Void) {
        lock.lock(); token &+= 1; let ticket = token; lock.unlock()
        control.async {
            let result = Result { try self.startSync(uid: uid, captureToken: ticket) }
            DispatchQueue.main.async { completion(result) }
        }
    }
    private func startSync(uid: String?, captureToken: UInt64) throws {
        lock.lock(); let current = token == captureToken; lock.unlock()
        guard current else { throw TalkError("Recording cancelled") }
        guard engine == nil else { throw TalkError("Microphone already recording") }
        guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
            throw TalkError("Microphone access is required. Run hey-talk permissions microphone.")
        }
        let audio = AVAudioEngine(), input = audio.inputNode
        if let uid {
            guard var id = AudioDevices.list().first(where: { $0.uid == uid })?.id else {
                throw TalkError("Selected microphone is disconnected. Use hey-talk input default or select another device.")
            }
            guard let unit = input.audioUnit,
                  AudioUnitSetProperty(unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0, &id, UInt32(MemoryLayout<AudioDeviceID>.size)) == noErr else {
                throw TalkError("Cannot select the microphone")
            }
        }
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0,
              let target = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false),
              let converter = AVAudioConverter(from: format, to: target) else { throw TalkError("Microphone has no usable audio format") }
        converter.primeMethod = .none
        lock.lock()
        guard token == captureToken else { lock.unlock(); throw TalkError("Recording cancelled") }
        samples = []; samples.reserveCapacity(PCMFrame.maxSamples)
        rms = 0; peak = 0; fault = nil; pending = 0
        lock.unlock()
        input.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buffer, _ in
            guard let self else { return }
            self.lock.lock()
            guard self.token == captureToken, self.fault == nil, self.pending < 16 else {
                let overflow = self.token == captureToken && self.pending >= 16 && self.fault == nil
                self.lock.unlock()
                if overflow { self.fail("Audio processing could not keep up; recording stopped", token: captureToken) }
                return
            }
            self.pending += 1
            self.lock.unlock()
            // No model work, file IO, UI updates, or waits on the realtime tap.
            guard let copy = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: buffer.frameLength) else {
                self.fail("Cannot allocate audio buffer", token: captureToken); return
            }
            copy.frameLength = buffer.frameLength
            let source = UnsafeMutableAudioBufferListPointer(buffer.mutableAudioBufferList)
            let destination = UnsafeMutableAudioBufferListPointer(copy.mutableAudioBufferList)
            for i in 0..<source.count {
                if let from = source[i].mData, let to = destination[i].mData {
                    memcpy(to, from, Int(source[i].mDataByteSize))
                }
            }
            self.processing.async {
                defer { self.lock.lock(); if self.token == captureToken { self.pending -= 1 }; self.lock.unlock() }
                let capacity = AVAudioFrameCount(ceil(Double(copy.frameLength) * 16_000 / format.sampleRate) + 32)
                guard let converted = AVAudioPCMBuffer(pcmFormat: target, frameCapacity: capacity) else { return }
                var supplied = false
                var error: NSError?
                let status = converter.convert(to: converted, error: &error) { _, state in
                    if supplied { state.pointee = .noDataNow; return nil }
                    supplied = true; state.pointee = .haveData; return copy
                }
                guard status != .error, let channel = converted.floatChannelData?[0] else {
                    self.fail(error?.localizedDescription ?? "Audio conversion failed", token: captureToken); return
                }
                let count = Int(converted.frameLength)
                guard count > 0 else { return }
                var level: Float = 0
                vDSP_rmsqv(channel, 1, &level, vDSP_Length(count))
                guard level.isFinite else { self.fail("Microphone returned invalid audio", token: captureToken); return }
                self.lock.lock()
                guard self.token == captureToken else { self.lock.unlock(); return }
                guard self.samples.count + count <= PCMFrame.maxSamples else {
                    self.lock.unlock(); self.fail("Five-minute recording limit reached. Stop and start a new dictation.", token: captureToken); return
                }
                self.samples.append(contentsOf: UnsafeBufferPointer(start: channel, count: count))
                self.rms = Double(level); self.peak = max(self.peak, level)
                self.lock.unlock()
            }
        }
        do { audio.prepare(); try audio.start() }
        catch { input.removeTap(onBus: 0); throw error }
        engine = audio
        lock.lock(); let cancelled = token != captureToken; lock.unlock()
        if cancelled { _ = try? stopSync(); throw TalkError("Recording cancelled") }
        routeRestarts = 0
        observer = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange, object: audio, queue: .main) { [weak self, weak audio] _ in
            guard let self, let audio else { return }
            self.control.async {
            guard self.engine === audio else { return }
            // Bluetooth output switches profile when a cue plays. Core Audio
            // can stop the graph although the microphone's format is unchanged.
            // Resume that graph without discarding already captured samples.
            if audio.isRunning { return }
            let current = audio.inputNode.outputFormat(forBus: 0)
            guard current.sampleRate == format.sampleRate, current.channelCount == format.channelCount, self.routeRestarts < 3 else {
                self.fail("Microphone connection changed. Start a new recording.", token: captureToken); return
            }
            self.routeRestarts += 1
            do { try audio.start() }
            catch { self.fail("Microphone could not resume: \(error.localizedDescription)", token: captureToken) }
            }
        }
    }
    private func fail(_ message: String, token: UInt64) {
        lock.lock()
        guard self.token == token, fault == nil else { lock.unlock(); return }
        fault = message
        lock.unlock()
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.lock.lock(); let current = self.token == token; self.lock.unlock()
            if current { self.onFailure?(message) }
        }
    }
    func stop(completion: @escaping (Result<[Float], Error>) -> Void) {
        control.async {
            let result = Result { try self.stopSync() }
            DispatchQueue.main.async { completion(result) }
        }
    }
    private func stopSync() throws -> [Float] {
        if let observer { NotificationCenter.default.removeObserver(observer); self.observer = nil }
        engine?.inputNode.removeTap(onBus: 0); engine?.stop(); engine = nil
        processing.sync {} // drain the final copied audio buffers before decoding
        lock.lock(); defer { lock.unlock() }
        let captured = samples; samples = []; rms = 0
        if let fault { throw TalkError(fault) }
        return captured
    }
    func cancel() {
        lock.lock(); token &+= 1; samples = []; rms = 0; lock.unlock()
        control.async { _ = try? self.stopSync() }
    }

    static func readFile(_ path: String) throws -> [Float] {
        let file = try AVAudioFile(forReading: URL(fileURLWithPath: path))
        let format = file.processingFormat
        guard Double(file.length) / format.sampleRate <= 300 else { throw TalkError("Audio file exceeds five minutes") }
        let target = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false)!
        guard let converter = AVAudioConverter(from: format, to: target) else { throw TalkError("Unsupported audio format") }
        let input = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 8192)!
        let output = AVAudioPCMBuffer(pcmFormat: target, frameCapacity: 8192)!
        var result: [Float] = []
        var readError: Error?
        while true {
            var error: NSError?
            let status = converter.convert(to: output, error: &error) { requested, state in
                let remaining = file.length - file.framePosition
                guard remaining > 0 else { state.pointee = .endOfStream; return nil }
                let count = min(input.frameCapacity, requested, AVAudioFrameCount(remaining))
                do { try file.read(into: input, frameCount: count) } catch { readError = error; state.pointee = .endOfStream; return nil }
                state.pointee = input.frameLength == 0 ? .endOfStream : .haveData
                return input.frameLength == 0 ? nil : input
            }
            if let error { throw error }; if let readError { throw readError }
            if let channel = output.floatChannelData?[0] {
                result.append(contentsOf: UnsafeBufferPointer(start: channel, count: Int(output.frameLength)))
            }
            guard result.count <= PCMFrame.maxSamples else { throw TalkError("Audio file exceeds five minutes") }
            if status == .endOfStream { break }
            if status == .error { throw TalkError("Cannot decode audio file") }
        }
        guard !result.isEmpty else { throw TalkError("Audio file is empty") }
        return result
    }
}
