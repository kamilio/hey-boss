import Foundation
import AVFoundation
import AudioToolbox
import CoreAudio
import Accelerate
import IOKit
import os
import TalkCore

struct InputDevice: Equatable {
    let id: AudioDeviceID
    let uid: String
    let name: String
    var json: [String: Any] { ["uid": uid, "name": name, "default": id == AudioDevices.defaultInput] }
}

enum AudioDevices {
    static func builtInFallback(for failed: InputDevice) -> InputDevice? {
        guard isBluetoothInput(failed.id), !isMuted(failed.id) else { return nil }
        let power = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("IOPMrootDomain"))
        guard power != 0 else { return nil }
        defer { IOObjectRelease(power) }
        let lidClosed = IORegistryEntryCreateCFProperty(power, "AppleClamshellState" as CFString,
                                                       kCFAllocatorDefault, 0)?.takeRetainedValue() as? Bool
        guard lidClosed != true else { return nil }
        return list().first { transport($0.id) == kAudioDeviceTransportTypeBuiltIn && !isMuted($0.id) }
    }
    private static func isMuted(_ device: AudioDeviceID) -> Bool {
        var address = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyMute, mScope: kAudioDevicePropertyScopeInput, mElement: kAudioObjectPropertyElementMain)
        var muted: UInt32 = 0, size = UInt32(MemoryLayout<UInt32>.size)
        return AudioObjectGetPropertyData(device, &address, 0, nil, &size, &muted) == noErr && muted != 0
    }
    static func input(uid: String?) throws -> InputDevice {
        let devices = list(), defaultID = defaultInput
        guard let device = devices.first(where: { device in uid.map { $0 == device.uid } ?? (device.id == defaultID) }) else {
            throw TalkError("Microphone unavailable. Check the system input in Sound settings.")
        }
        return device
    }
    static func isBluetoothInput(_ device: AudioDeviceID) -> Bool {
        let value = transport(device)
        return value == kAudioDeviceTransportTypeBluetooth || value == kAudioDeviceTransportTypeBluetoothLE
    }
    private static func transport(_ device: AudioDeviceID) -> UInt32? {
        var address = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyTransportType, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var transport: UInt32 = 0, size = UInt32(MemoryLayout<UInt32>.size)
        guard AudioObjectGetPropertyData(device, &address, 0, nil, &size, &transport) == noErr else { return nil }
        return transport
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

final class AudioCapture {
    private let logger = Logger(subsystem: "local.hey-talk", category: "capture")
    typealias StreamFactory = (InputDevice, DispatchQueue, @escaping (CMSampleBuffer) -> Void, @escaping (String) -> Void) throws -> MicrophoneStream
    private let resolveInput: (String?) throws -> InputDevice
    private let fallbackInput: (InputDevice) -> InputDevice?
    private let makeStream: StreamFactory
    private let now: () -> TimeInterval
    private let control = DispatchQueue(label: "hey-talk.audio-control", qos: .userInitiated)
    private let processing = DispatchQueue(label: "hey-talk.audio", qos: .userInteractive)
    private var stream: MicrophoneStream?
    private var inputObserver: AudioInputObserver?
    private var watchdog: DispatchSourceTimer?
    // These fields belong to the control queue.
    private var recordingToken: UInt64?
    private var requestedUID: String?
    private var fallbackRoute: (source: InputDevice, input: InputDevice)?
    private var triedFallback = false
    private var recoveryStartedAt: TimeInterval?
    private var recoveryReason = "Microphone unavailable. Check the system input in Sound settings."
    private var nextAttemptAt: TimeInterval = 0
    private var attempts = 0
    private var idleUntil: TimeInterval?
    private let lock = NSLock()
    // Audio callbacks and the UI share the following fields under the lock.
    private var samples: [Float] = []
    private var rms: Double = 0
    private var peak: Float = 0
    private var fault: String?
    private var token: UInt64 = 0
    private var streamID: UInt64 = 0
    private var bluetooth = false
    private var device: InputDevice?
    private var fallbackDevice: InputDevice?
    private var ready = false
    private var restarts = 0
    private var lastAudioAt: TimeInterval = 0
    private var lastSignalAt: TimeInterval = 0
    private var signalSince: TimeInterval?
    private var acceptingAudio = false
    private var keepingAlive = false
    var onFailure: ((String) -> Void)?
    var onReadinessChanged: ((Bool) -> Void)?
    var level: Double { lock.lock(); defer { lock.unlock() }; return rms }
    var sampleCount: Int { lock.lock(); defer { lock.unlock() }; return samples.count }
    var maximumLevel: Float { lock.lock(); defer { lock.unlock() }; return peak }
    var usesBluetooth: Bool { lock.lock(); defer { lock.unlock() }; return bluetooth }
    var activeInput: InputDevice? { lock.lock(); defer { lock.unlock() }; return device }
    var fallbackSource: InputDevice? { lock.lock(); defer { lock.unlock() }; return fallbackDevice }
    var isReady: Bool { lock.lock(); defer { lock.unlock() }; return ready }
    var restartCount: Int { lock.lock(); defer { lock.unlock() }; return restarts }
    var isKeepingAlive: Bool { lock.lock(); defer { lock.unlock() }; return keepingAlive }

    init(resolveInput: @escaping (String?) throws -> InputDevice = AudioDevices.input,
         fallbackInput: @escaping (InputDevice) -> InputDevice? = AudioDevices.builtInFallback,
         makeStream: @escaping StreamFactory = { try SystemMicrophoneStream(device: $0, queue: $1, receive: $2, interrupted: $3) },
         now: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }) {
        self.resolveInput = resolveInput; self.fallbackInput = fallbackInput; self.makeStream = makeStream; self.now = now
    }

    func start(uid: String?, completion: @escaping (Result<Void, Error>) -> Void) {
        lock.lock(); token &+= 1; acceptingAudio = false; let ticket = token; lock.unlock()
        control.async {
            guard self.isCurrent(ticket) else { DispatchQueue.main.async { completion(.failure(TalkError("Recording cancelled"))) }; return }
            self.lock.lock(); let recentAudio = self.now() - self.lastAudioAt < 3; self.lock.unlock()
            let reuse = self.idleUntil.map { self.now() < $0 } == true && self.stream != nil && recentAudio
                && (try? self.resolveInput(uid)) == self.activeInput
            if !reuse { self.endCapture() }
            // Drain queued idle buffers before accepting a new recording.
            self.processing.sync {}
            self.idleUntil = nil
            self.recordingToken = ticket; self.requestedUID = uid
            self.fallbackRoute = nil; self.triedFallback = false
            self.lock.lock()
            guard self.token == ticket else {
                self.lock.unlock(); self.endCapture()
                DispatchQueue.main.async { completion(.failure(TalkError("Recording cancelled"))) }; return
            }
            self.samples = []; self.samples.reserveCapacity(PCMFrame.maxSamples)
            self.fallbackDevice = nil
            self.rms = 0; self.peak = 0; self.fault = nil; self.ready = false; self.restarts = 0
            self.acceptingAudio = true; self.keepingAlive = false
            self.lastSignalAt = self.now(); self.signalSince = nil
            self.lock.unlock()
            self.recoveryStartedAt = nil; self.nextAttemptAt = 0; self.attempts = reuse ? 1 : 0
            if self.watchdog == nil {
                self.inputObserver = AudioInputObserver(queue: self.control) { [weak self] in self?.refresh() }
                let timer = DispatchSource.makeTimerSource(queue: self.control)
                timer.schedule(deadline: .now() + 0.5, repeating: 0.5)
                timer.setEventHandler { [weak self] in self?.refreshSync() }
                self.watchdog = timer; timer.resume()
            }
            self.refreshSync()
            self.waitForAudio(token: ticket, deadline: self.now() + 12, completion: completion)
        }
    }
    func waitForAudio(token: UInt64, deadline: TimeInterval, completion: @escaping (Result<Void, Error>) -> Void) {
        lock.lock()
        let current = self.token == token, ready = self.ready, fault = self.fault
        lock.unlock()
        let result: Result<Void, Error>
        if !current { result = .failure(TalkError("Recording cancelled")) }
        else if let fault { result = .failure(TalkError(fault)) }
        else if ready { result = .success(()) }
        else if now() >= deadline {
            let message = "No sound from the microphone. Check its mute control and the system input in Sound settings."
            fail(message, token: token)
            result = .failure(TalkError(message))
        } else {
            control.asyncAfter(deadline: .now() + 0.02) { self.waitForAudio(token: token, deadline: deadline, completion: completion) }
            return
        }
        DispatchQueue.main.async { completion(result) }
    }
    private func isCurrent(_ ticket: UInt64, stream: UInt64? = nil) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return token == ticket && (stream == nil || streamID == stream)
    }
    func refresh() { control.async { self.refreshSync() } }
    private func refreshSync() {
        if let deadline = idleUntil {
            lock.lock(); let stale = now() - lastAudioAt >= 3; lock.unlock()
            if now() >= deadline || stale || (try? resolveInput(requestedUID)) != activeInput { endCapture() }
            return
        }
        guard let ticket = recordingToken, isCurrent(ticket) else { return }
        let time = now()
        lock.lock()
        let failed = fault != nil
        let healthy = signalSince.map { time - $0 >= 1 && time - lastAudioAt < 1 && time - lastSignalAt < 1 } ?? false
        lock.unlock()
        guard !failed else { return }
        if stream != nil, healthy { recoveryStartedAt = nil }
        if let began = recoveryStartedAt, time - began >= 12 {
            fail(recoveryReason, token: ticket); return
        }
        do {
            // Resolve the system default anew, including between retries. Never fall
            // back to an unrelated device when an explicit input was selected.
            var input = try resolveInput(requestedUID)
            if let route = fallbackRoute {
                if input == route.source, fallbackInput(route.source) == route.input { input = route.input }
                else { fallbackRoute = nil }
            }
            lock.lock()
            let sameInput = device == input
            let noBuffers = time - lastAudioAt >= 3
            let noSignal = time - lastSignalAt >= 3
            lock.unlock()
            if stream != nil, sameInput {
                if noBuffers || noSignal {
                    // Reopening cannot reclaim AirPods owned by another device.
                    // After one retry, default mode may use the unmuted built-in
                    // microphone for this recording. Explicit choices stay pinned.
                    if requestedUID == nil, !triedFallback, attempts >= 2,
                       let replacement = fallbackInput(input), replacement != input {
                        triedFallback = true
                        fallbackRoute = (input, replacement)
                        input = replacement
                        logger.notice("Default microphone remained silent after retry; switching to built-in input")
                    }
                    beginRecovery(noBuffers ? "The microphone stopped sending audio. Check the system input in Sound settings."
                                  : "No sound from the microphone. Check its mute control and the system input in Sound settings.", token: ticket)
                } else { return }
            } else if stream != nil {
                beginRecovery("Microphone unavailable after the input changed. Check the system input in Sound settings.", token: ticket)
                // A deliberate route change can open immediately.
                nextAttemptAt = time
            }
            guard stream == nil, time >= nextAttemptAt else { return }
            lock.lock()
            streamID &+= 1; let epoch = streamID
            device = input; ready = false; rms = 0
            fallbackDevice = fallbackRoute?.source
            lastAudioAt = time; lastSignalAt = time; signalSince = nil
            restarts = attempts; lock.unlock()
            attempts += 1
            logger.info("Opening microphone attempt \(self.attempts, privacy: .public), fallback \(self.fallbackRoute != nil, privacy: .public)")
            let bluetooth = AudioDevices.isBluetoothInput(input.id)
            lock.lock(); self.bluetooth = bluetooth; lock.unlock()
            let opened = try makeStream(input, processing,
                { [weak self] in self?.receive($0, stream: epoch) },
                { [weak self] in self?.streamInterrupted($0, stream: epoch) })
            stream = opened
            try opened.start()
            if !isCurrent(ticket) { closeStream() }
        } catch is MicrophoneAccessError {
            fail(MicrophoneAccessError.denied.localizedDescription, token: ticket)
        } catch {
            beginRecovery(error.localizedDescription, token: ticket)
        }
    }
    private func beginRecovery(_ message: String, token: UInt64) {
        logger.notice("Recovering microphone: \(message, privacy: .public)")
        if recoveryStartedAt == nil { recoveryStartedAt = now() }
        recoveryReason = message
        closeStream()
        nextAttemptAt = now() + 0.25
        notifyReadiness(false, token: token)
    }
    private func recover(_ message: String, token: UInt64, stream: UInt64) {
        control.async {
            guard self.recordingToken == token, self.isCurrent(token, stream: stream) else { return }
            self.beginRecovery(message, token: token)
        }
    }
    private func streamInterrupted(_ message: String, stream: UInt64) {
        control.async {
            self.lock.lock(); let current = self.streamID == stream; self.lock.unlock()
            guard current else { return }
            if let ticket = self.recordingToken, self.isCurrent(ticket) { self.beginRecovery(message, token: ticket) }
            else { self.endCapture() }
        }
    }
    private func receive(_ buffer: CMSampleBuffer, stream: UInt64) {
        lock.lock()
        guard streamID == stream, fault == nil else { lock.unlock(); return }
        let ticket = token
        guard acceptingAudio else {
            // Keep the Bluetooth route open, but never retain, meter, or decode
            // audio between dictations. Fresh buffers prove the stream is alive.
            lastAudioAt = now(); lock.unlock(); return
        }
        lock.unlock()
        consume(buffer, token: ticket, stream: stream)
    }
    func consume(_ buffer: CMSampleBuffer, token: UInt64, stream: UInt64 = 0) {
        lock.lock(); let current = self.token == token && streamID == stream && fault == nil; lock.unlock()
        guard current, let description = CMSampleBufferGetFormatDescription(buffer),
              let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(description) else { return }
        let format = asbd.pointee
        guard format.mFormatID == kAudioFormatLinearPCM, format.mSampleRate == 16_000,
              format.mChannelsPerFrame == 1, format.mBitsPerChannel == 32,
              format.mFormatFlags & kAudioFormatFlagIsFloat != 0 else {
            recover("Microphone returned an unsupported audio format", token: token, stream: stream); return
        }
        let count = CMSampleBufferGetNumSamples(buffer)
        guard count > 0, count <= PCMFrame.rate else { return }
        var values = [Float](repeating: 0, count: count)
        let status = values.withUnsafeMutableBytes { raw -> OSStatus in
            var list = AudioBufferList(mNumberBuffers: 1, mBuffers: AudioBuffer(mNumberChannels: 1, mDataByteSize: UInt32(raw.count), mData: raw.baseAddress))
            return CMSampleBufferCopyPCMDataIntoAudioBufferList(buffer, at: 0, frameCount: Int32(count), into: &list)
        }
        guard status == noErr else { recover("Cannot read microphone samples", token: token, stream: stream); return }
        var level: Float = 0
        vDSP_rmsqv(values, 1, &level, vDSP_Length(count))
        guard level.isFinite else { recover("Microphone returned invalid audio", token: token, stream: stream); return }
        lock.lock()
        guard self.token == token, streamID == stream, fault == nil else { lock.unlock(); return }
        guard samples.count + count <= PCMFrame.maxSamples else {
            lock.unlock(); fail("Five-minute recording limit reached. Stop and start a new dictation.", token: token); return
        }
        samples.append(contentsOf: values); rms = Double(level); peak = max(peak, level)
        let time = now(), becameReady = !ready && level > 0
        lastAudioAt = time
        if level > 0 {
            if signalSince == nil || time - lastSignalAt >= 1 { signalSince = time }
            lastSignalAt = time; ready = true
        } else if time - lastSignalAt >= 1 { signalSince = nil }
        lock.unlock()
        if becameReady {
            logger.info("Microphone is delivering nonzero audio")
            notifyReadiness(true, token: token)
        }
    }
    private func notifyReadiness(_ ready: Bool, token: UInt64) {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.lock.lock(); let current = self.token == token && self.ready == ready; self.lock.unlock()
            if current { self.onReadinessChanged?(ready) }
        }
    }
    private func fail(_ message: String, token: UInt64) {
        lock.lock()
        guard self.token == token, fault == nil else { lock.unlock(); return }
        fault = message; ready = false; lock.unlock()
        logger.error("Microphone failed: \(message, privacy: .public)")
        control.async { if self.recordingToken == token { self.endCapture() } }
        DispatchQueue.main.async { [weak self] in
            guard let self, self.isCurrent(token) else { return }
            self.onFailure?(message)
        }
    }
    private func closeStream() {
        lock.lock(); streamID &+= 1; ready = false; rms = 0; device = nil; keepingAlive = false; lock.unlock()
        stream?.stop(); stream = nil
        processing.sync {} // Reject and drain old-route callbacks before reopening.
    }
    private func endCapture() {
        watchdog?.cancel(); watchdog = nil; inputObserver = nil
        recordingToken = nil; idleUntil = nil
        closeStream()
    }
    private func keepConnectionAlive() {
        recordingToken = nil
        lock.lock()
        let healthy = stream != nil && fault == nil && now() - lastAudioAt < 3
        acceptingAudio = false; ready = false; keepingAlive = healthy
        lock.unlock()
        if healthy { idleUntil = now() + 300 }
        else { endCapture() }
    }
    func stop(completion: @escaping (Result<[Float], Error>) -> Void) {
        lock.lock(); let ticket = token; lock.unlock()
        control.async {
            guard self.isCurrent(ticket) else {
                DispatchQueue.main.async { completion(.failure(TalkError("Recording cancelled"))) }; return
            }
            self.lock.lock()
            guard self.token == ticket else {
                self.lock.unlock()
                DispatchQueue.main.async { completion(.failure(TalkError("Recording cancelled"))) }; return
            }
            self.token &+= 1; self.acceptingAudio = false; self.ready = false
            let captured = self.samples, fault = self.fault
            self.samples = []; self.rms = 0
            self.lock.unlock()
            self.keepConnectionAlive()
            let result: Result<[Float], Error> = fault.map { .failure(TalkError($0)) } ?? .success(captured)
            DispatchQueue.main.async { completion(result) }
        }
    }
    func cancel(keepWarm: Bool = false) {
        lock.lock(); token &+= 1; let ticket = token
        acceptingAudio = false; samples = []; rms = 0; ready = false; lock.unlock()
        control.async {
            guard self.isCurrent(ticket) else { return }
            if keepWarm { self.keepConnectionAlive() } else { self.endCapture() }
        }
    }
    deinit { watchdog?.cancel(); stream?.stop() }

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
