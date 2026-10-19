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
    static func isBluetoothInput(uid: String?) -> Bool {
        let device = uid.flatMap { selected in list().first(where: { $0.uid == selected })?.id } ?? defaultInput
        var address = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyTransportType, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var transport: UInt32 = 0, size = UInt32(MemoryLayout<UInt32>.size)
        guard AudioObjectGetPropertyData(device, &address, 0, nil, &size, &transport) == noErr else { return false }
        return transport == kAudioDeviceTransportTypeBluetooth || transport == kAudioDeviceTransportTypeBluetoothLE
    }
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

private final class CaptureReceiver: NSObject, AVCaptureAudioDataOutputSampleBufferDelegate {
    let receive: (CMSampleBuffer) -> Void
    init(receive: @escaping (CMSampleBuffer) -> Void) { self.receive = receive }
    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        receive(sampleBuffer)
    }
}

final class AudioCapture {
    private var captureSession: AVCaptureSession?
    private var receiver: CaptureReceiver?
    private var output: AVCaptureAudioDataOutput?
    private var observers: [NSObjectProtocol] = []
    private var watchdog: DispatchSourceTimer?
    private let control = DispatchQueue(label: "hey-talk.audio-control", qos: .userInitiated)
    private let processing = DispatchQueue(label: "hey-talk.audio", qos: .userInteractive)
    private let lock = NSLock()
    private var samples: [Float] = []
    private var rms: Double = 0
    private var peak: Float = 0
    private var fault: String?
    private var token: UInt64 = 0
    private var bluetooth = false
    private var lastAudioAt: TimeInterval = 0
    var onFailure: ((String) -> Void)?
    var level: Double { lock.lock(); defer { lock.unlock() }; return rms }
    var sampleCount: Int { lock.lock(); defer { lock.unlock() }; return samples.count }
    var maximumLevel: Float { lock.lock(); defer { lock.unlock() }; return peak }
    var usesBluetooth: Bool { lock.lock(); defer { lock.unlock() }; return bluetooth }

    func start(uid: String?, completion: @escaping (Result<Void, Error>) -> Void) {
        lock.lock(); token &+= 1; let ticket = token; lock.unlock()
        control.async {
            do {
                try self.startSync(uid: uid, captureToken: ticket)
                self.waitForAudio(token: ticket, deadline: ProcessInfo.processInfo.systemUptime + 5, completion: completion)
            } catch { DispatchQueue.main.async { completion(.failure(error)) } }
        }
    }
    private func waitForAudio(token: UInt64, deadline: TimeInterval, completion: @escaping (Result<Void, Error>) -> Void) {
        lock.lock()
        let current = self.token == token, ready = samples.count >= 2400, fault = self.fault
        lock.unlock()
        let result: Result<Void, Error>
        if !current { result = .failure(TalkError("Recording cancelled")) }
        else if let fault { result = .failure(TalkError(fault)) }
        else if ready { result = .success(()) }
        else if ProcessInfo.processInfo.systemUptime >= deadline {
            result = .failure(TalkError("The microphone opened but sent no audio. Reconnect it or choose another input."))
        } else {
            control.asyncAfter(deadline: .now() + 0.02) { self.waitForAudio(token: token, deadline: deadline, completion: completion) }
            return
        }
        DispatchQueue.main.async { completion(result) }
    }
    private func startSync(uid: String?, captureToken: UInt64) throws {
        lock.lock(); let current = token == captureToken; lock.unlock()
        guard current else { throw TalkError("Recording cancelled") }
        guard captureSession == nil else { throw TalkError("Microphone already recording") }
        guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
            throw TalkError("Microphone access is required. Run hey-talk permissions microphone.")
        }
        // Capture-only sessions do not create an output audio graph. In particular,
        // they avoid AVAudioEngine's stale Bluetooth output format after route changes.
        let selectedUID = uid ?? AudioDevices.list().first(where: { $0.id == AudioDevices.defaultInput })?.uid
        let devices = AVCaptureDevice.DiscoverySession(deviceTypes: [.microphone, .external], mediaType: .audio, position: .unspecified).devices
        guard let device = selectedUID.flatMap({ selected in devices.first(where: { $0.uniqueID == selected }) }) else {
            throw TalkError("Selected microphone is disconnected. Use hey-talk input default or select another device.")
        }
        let session = AVCaptureSession()
        let input = try AVCaptureDeviceInput(device: device)
        let output = AVCaptureAudioDataOutput()
        output.audioSettings = [AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: 16_000,
                                AVNumberOfChannelsKey: 1, AVLinearPCMBitDepthKey: 32,
                                AVLinearPCMIsFloatKey: true, AVLinearPCMIsNonInterleaved: false]
        let receiver = CaptureReceiver { [weak self] buffer in self?.consume(buffer, token: captureToken) }
        output.setSampleBufferDelegate(receiver, queue: processing)
        session.beginConfiguration()
        guard session.canAddInput(input), session.canAddOutput(output) else {
            session.commitConfiguration(); throw TalkError("Cannot configure microphone capture")
        }
        session.addInput(input); session.addOutput(output); session.commitConfiguration()
        let usesBluetooth = AudioDevices.isBluetoothInput(uid: selectedUID)
        lock.lock()
        guard token == captureToken else { lock.unlock(); throw TalkError("Recording cancelled") }
        samples = []; samples.reserveCapacity(PCMFrame.maxSamples)
        rms = 0; peak = 0; fault = nil; bluetooth = usesBluetooth
        lastAudioAt = ProcessInfo.processInfo.systemUptime
        lock.unlock()
        captureSession = session; self.output = output; self.receiver = receiver
        for name in [AVCaptureSession.runtimeErrorNotification, AVCaptureSession.wasInterruptedNotification] {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: session, queue: nil) { [weak self] notification in
                let error = notification.userInfo?[AVCaptureSessionErrorKey] as? Error
                self?.fail(error?.localizedDescription ?? "Microphone capture was interrupted. Start a new recording.", token: captureToken)
            })
        }
        session.startRunning()
        lock.lock(); let cancelled = token != captureToken; lock.unlock()
        if cancelled { _ = try? stopSync(); throw TalkError("Recording cancelled") }
        guard session.isRunning else { _ = try? stopSync(); throw TalkError("Microphone could not start") }
        let timer = DispatchSource.makeTimerSource(queue: control)
        timer.schedule(deadline: .now() + 5, repeating: 0.5)
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            self.lock.lock(); let stale = self.token == captureToken && ProcessInfo.processInfo.systemUptime - self.lastAudioAt > 3; self.lock.unlock()
            if stale { self.fail("The microphone stopped sending audio. Reconnect it or choose another input.", token: captureToken) }
        }
        watchdog = timer; timer.resume()
    }
    private func consume(_ buffer: CMSampleBuffer, token: UInt64) {
        lock.lock(); let current = self.token == token && fault == nil; lock.unlock()
        guard current, let description = CMSampleBufferGetFormatDescription(buffer),
              let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(description) else { return }
        let format = asbd.pointee
        guard format.mFormatID == kAudioFormatLinearPCM, format.mSampleRate == 16_000,
              format.mChannelsPerFrame == 1, format.mBitsPerChannel == 32,
              format.mFormatFlags & kAudioFormatFlagIsFloat != 0 else {
            fail("Microphone returned an unsupported audio format", token: token); return
        }
        let count = CMSampleBufferGetNumSamples(buffer)
        guard count > 0, count <= PCMFrame.rate else { return }
        var values = [Float](repeating: 0, count: count)
        let status = values.withUnsafeMutableBytes { raw -> OSStatus in
            var list = AudioBufferList(mNumberBuffers: 1, mBuffers: AudioBuffer(mNumberChannels: 1, mDataByteSize: UInt32(raw.count), mData: raw.baseAddress))
            return CMSampleBufferCopyPCMDataIntoAudioBufferList(buffer, at: 0, frameCount: Int32(count), into: &list)
        }
        guard status == noErr else { fail("Cannot read microphone samples", token: token); return }
        var level: Float = 0
        vDSP_rmsqv(values, 1, &level, vDSP_Length(count))
        guard level.isFinite else { fail("Microphone returned invalid audio", token: token); return }
        lock.lock()
        guard self.token == token else { lock.unlock(); return }
        guard samples.count + count <= PCMFrame.maxSamples else {
            lock.unlock(); fail("Five-minute recording limit reached. Stop and start a new dictation.", token: token); return
        }
        samples.append(contentsOf: values); rms = Double(level); peak = max(peak, level)
        lastAudioAt = ProcessInfo.processInfo.systemUptime
        lock.unlock()
    }
    private func fail(_ message: String, token: UInt64) {
        lock.lock()
        guard self.token == token, fault == nil else { lock.unlock(); return }
        fault = message; lock.unlock()
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
        watchdog?.cancel(); watchdog = nil
        observers.forEach(NotificationCenter.default.removeObserver); observers = []
        captureSession?.stopRunning()
        output?.setSampleBufferDelegate(nil, queue: nil)
        processing.sync {} // Drain the final callbacks before decoding.
        output = nil; receiver = nil; captureSession = nil
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
