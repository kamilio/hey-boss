import XCTest
import AVFoundation
import TalkCore
@testable import HeyTalk

final class AudioTests: XCTestCase {
    func testDictationReturnsMicrophoneToListeningAndSpeakingConversation() throws {
        guard let binary = ProcessInfo.processInfo.environment["HEY_BOSS_HANDOFF_TEST_BINARY"] else {
            throw XCTSkip("Run via tools/conversation_checks.sh with the desktop audit binary")
        }
        let directory = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
            .appendingPathComponent(".build/handoff-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let utterance = directory.appendingPathComponent("dictation.aiff")
        let say = Process()
        say.executableURL = URL(fileURLWithPath: "/usr/bin/say")
        say.arguments = ["-v", "Samantha", "-o", utterance.path, "These words belong in dictation."]
        try say.run(); say.waitUntilExit()
        let dictated = try AudioCapture.readFile(utterance.path)
        let peer = Process(), input = Pipe(), output = Pipe()
        peer.executableURL = URL(fileURLWithPath: binary)
        peer.environment = ProcessInfo.processInfo.environment.merging(["HEY_BOSS_AUDIT_DICTATION_HANDOFF": directory.path, "HEY_BOSS_HANDOFF_PEER": "1"]) { _, new in new }
        peer.standardInput = input; peer.standardOutput = output
        try peer.run()
        defer { if peer.isRunning { peer.terminate() } }
        let rig = try CaptureRig(buffer: firstBuffer())
        let capture = rig.capture()
        let history = DictationHistory(directory: directory)
        let service = TalkService(headless: true, capture: capture, history: history) { request in
            guard ["conversation_pause", "conversation_resume"].contains(request["command"] as? String) else { return nil }
            if request["command"] as? String == "conversation_resume" {
                XCTAssertTrue(rig.streams.last?.stopped == true, "Chat must not resume while the dictation stream is warm")
            }
            var data = try JSONSerialization.data(withJSONObject: request); data.append(10)
            try input.fileHandleForWriting.write(contentsOf: data)
            let line = try Wire.line(from: output.fileHandleForReading.fileDescriptor, timeout: 20)
            return try JSONSerialization.jsonObject(with: line) as? [String: Any]
        }
        func status() -> [String: Any] {
            var value: [String: Any] = [:]
            service.command(["command": "status"]) { value = $0 }
            return value
        }
        var keyboard = HotkeyState()
        func pressHyperR() {
            service.handleShortcut(keyboard.keyDown(code: 15, hyper: true, repeated: false))
            _ = keyboard.keyUp(code: 15)
        }
        for phase in ["listening", "speaking"] {
            service.applyConversation(["active": true, "phase": phase])
            pressHyperR()
            let ready = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in status()["microphone_ready"] as? Bool == true }, object: nil)
            wait(for: [ready], timeout: 25)
            XCTAssertNil(status()["conversation"])
            let stream = try XCTUnwrap(rig.streams.last)
            for offset in stride(from: 0, to: dictated.count, by: 320) {
                stream.receive(try sampleBuffer(Array(dictated[offset..<min(offset + 320, dictated.count)])))
            }
            XCTAssertGreaterThan(capture.sampleCount, 2400)
            pressHyperR()
            eventually { (status()["conversation"] as? [String: Any])?["phase"] as? String == phase }
        }
        history.finishPendingWrites()
        for index in 1...2 {
            let saved = try AudioCapture.readFile(directory.appendingPathComponent("recordings/recording-\(index).wav").path)
            XCTAssertTrue(saved.suffix(dictated.count).elementsEqual(dictated), "Only the dictation recorder receives this utterance")
        }
        eventually { !peer.isRunning }
        if !peer.isRunning { XCTAssertEqual(peer.terminationStatus, 0) }
    }

    func testAutomaticCaptureUsesConversationDetectorAndKeepsOnlyTheSpokenTurn() throws {
        let directory = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
            .appendingPathComponent(".build/auto-speech-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let file = directory.appendingPathComponent("speech.aiff")
        let say = Process()
        say.executableURL = URL(fileURLWithPath: "/usr/bin/say")
        say.arguments = ["-v", "Samantha", "-o", file.path, "Hey Talk records my voice and turns it into text."]
        try say.run(); say.waitUntilExit()
        XCTAssertEqual(say.terminationStatus, 0)
        let voice = try AudioCapture.readFile(file.path)
        let noise = try firstBuffer(value: 0.0001)
        let rig = try CaptureRig(buffer: noise), capture = rig.capture()
        defer { capture.cancel() }
        var began = 0, ended = 0, recorded: [Float] = []
        let stopped = expectation(description: "Silence stops capture automatically")
        capture.onSpeechBegan = { began += 1 }
        capture.onSpeechEnded = {
            ended += 1
            capture.stop { result in
                do { recorded = try result.get() } catch { XCTFail(error.localizedDescription) }
                stopped.fulfill()
            }
        }
        defer { capture.onSpeechEnded = nil }
        capture.onFailure = { XCTFail($0) }
        let started = expectation(description: "Listening microphone ready")
        capture.start(uid: nil, detectSpeech: true) { result in
            if case .failure(let error) = result { XCTFail(error.localizedDescription) }
            started.fulfill()
        }
        wait(for: [started], timeout: 3)
        let stream = try XCTUnwrap(rig.streams.first)
        // Feed in real-time chunks so the nonblocking audio pipe stays bounded.
        func feed(_ values: [Float]) throws {
            for offset in stride(from: 0, to: values.count, by: 320) {
                let chunk = Array(values[offset..<min(offset + 320, values.count)])
                stream.receive(try sampleBuffer(chunk))
                RunLoop.current.run(until: Date().addingTimeInterval(0.02))
            }
        }
        try feed(Array(repeating: 0.0001, count: 32_000))
        XCTAssertEqual(capture.sampleCount, 0, "Listening audio must not become a recording")
        XCTAssertEqual(began, 0)
        try feed(voice)
        XCTAssertEqual(began, 1)
        try feed(Array(repeating: 0.0001, count: 16_000))
        XCTAssertEqual(ended, 0, "A thinking pause stays in the same turn")
        try feed(voice)
        XCTAssertEqual(began, 1)
        try feed(Array(repeating: 0.0001, count: 48_000))
        eventually { ended == 1 }
        wait(for: [stopped], timeout: 2)
        let opening = recorded.prefix { abs($0) < 0.001 }.count
        XCTAssertGreaterThanOrEqual(opening, 12_000, "Keep at least 750 ms before the opening word, including audio the detector has not classified as speech")
        XCTAssertLessThanOrEqual(opening, 24_000, "The rolling buffer must not retain the entire listening session")
        XCTAssertGreaterThan(recorded.count, voice.count * 2, "The complete utterance includes the buffered opening and pause")
        try feed(voice)
        XCTAssertEqual(capture.sampleCount, 0, "Audio after submission must not leak into a completed turn")
        XCTAssertEqual(began, 1)
        if let app = ProcessInfo.processInfo.environment["HEY_TALK_TEST_APP"] {
            let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false)!
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(recorded.count))!
            buffer.frameLength = buffer.frameCapacity
            recorded.withUnsafeBufferPointer { buffer.floatChannelData![0].update(from: $0.baseAddress!, count: $0.count) }
            let audio = directory.appendingPathComponent("detected.wav")
            try AVAudioFile(forWriting: audio, settings: format.settings).write(from: buffer)
            let decode = Process(), output = Pipe()
            decode.executableURL = URL(fileURLWithPath: app).appendingPathComponent("Contents/MacOS/hey-talk")
            decode.arguments = ["transcribe-file", audio.path, "--json"]
            decode.standardOutput = output
            try decode.run()
            let response = output.fileHandleForReading.readDataToEndOfFile()
            decode.waitUntilExit()
            XCTAssertEqual(decode.terminationStatus, 0)
            let json = try JSONSerialization.jsonObject(with: response) as? [String: Any]
            let transcript = try XCTUnwrap(json?["text"] as? String).lowercased()
            XCTAssertTrue(transcript.hasPrefix("hey talk"), transcript)
            XCTAssertTrue(transcript.contains("records my voice"), transcript)
            XCTAssertTrue(transcript.contains("turns it into text"), transcript)
        }
    }

    func testSilentDefaultRetriesOnceThenUsesFallbackUntilTheDefaultChanges() throws {
        let sound = try firstBuffer(), silence = try firstBuffer(value: 0)
        let rig = try CaptureRig(buffer: sound)
        let capture = rig.capture(fallback: { _ in CaptureRig.builtIn })
        defer { capture.cancel() }
        start(capture)
        let original = rig.streams[0]
        rig.advance(3.1); original.receive(silence); capture.refresh()
        eventually { original.stopped }
        rig.advance(0.5); capture.refresh()
        eventually { rig.streams.count == 2 && capture.isReady }
        XCTAssertEqual(capture.activeInput, CaptureRig.headset, "Retry the selected route before falling back")
        rig.advance(3.1); rig.streams[1].receive(silence); capture.refresh()
        eventually { rig.streams[1].stopped }
        rig.advance(0.5); capture.refresh()
        eventually { capture.activeInput == CaptureRig.builtIn && capture.isReady }
        XCTAssertEqual(capture.sampleCount, 800, "Recovery must preserve audio from before the route failed")
        original.receive(sound)
        XCTAssertEqual(capture.sampleCount, 800, "Old route callbacks must stay rejected")
        rig.advance(0.5); rig.streams[2].receive(sound); capture.refresh()
        let stopped = expectation(description: "Fallback remains selected")
        capture.stop { result in
            XCTAssertEqual(try? result.get().count, 960)
            stopped.fulfill()
        }
        wait(for: [stopped], timeout: 2)
        XCTAssertEqual(rig.streams.count, 3, "Do not oscillate back to the silent default")
        start(capture)
        XCTAssertEqual(capture.activeInput, CaptureRig.headset, "Fallback must not overwrite the next recording's preference")
    }

    func testSilentStartupCanBecomeReadyOnFallback() throws {
        let rig = try CaptureRig(buffer: firstBuffer(value: 0))
        let capture = rig.capture(fallback: { _ in CaptureRig.builtIn })
        defer { capture.cancel() }
        let started = expectation(description: "Fallback delivers the first sound")
        capture.start(uid: nil) { result in
            if case .failure(let error) = result { XCTFail(error.localizedDescription) }
            started.fulfill()
        }
        eventually { rig.streams.count == 1 }
        for index in 0..<2 {
            rig.advance(3.1); capture.refresh()
            eventually { rig.streams[index].stopped }
            rig.advance(0.5); capture.refresh()
            eventually { rig.streams.count == index + 2 }
        }
        XCTAssertEqual(capture.activeInput, CaptureRig.builtIn)
        rig.streams[2].receive(try firstBuffer())
        wait(for: [started], timeout: 2)
        XCTAssertTrue(capture.isReady)
        rig.input = CaptureRig.headsetReplacement
        capture.refresh()
        eventually { capture.activeInput == CaptureRig.headsetReplacement }
        XCTAssertTrue(rig.streams[2].stopped, "A deliberate default change still takes precedence")
    }

    func testExplicitMicrophoneNeverUsesFallback() throws {
        let rig = try CaptureRig(buffer: firstBuffer())
        let capture = rig.capture(fallback: { _ in XCTFail("Explicit input must stay selected"); return CaptureRig.builtIn })
        defer { capture.cancel() }
        start(capture, uid: CaptureRig.headset.uid)
        for index in 0..<2 {
            rig.advance(3.1); capture.refresh()
            eventually { rig.streams[index].stopped }
            rig.advance(0.5); capture.refresh()
            eventually { rig.streams.count == index + 2 && capture.isReady }
        }
        XCTAssertEqual(capture.activeInput, CaptureRig.headset)
    }

    func testStoppedMicrophoneIsReusedForFiveMinutesWithoutKeepingIdleAudio() throws {
        let rig = try CaptureRig(buffer: firstBuffer()), idle = try firstBuffer(value: 0.75)
        let capture = rig.capture()
        defer { capture.cancel() }
        start(capture)
        let stream = try XCTUnwrap(rig.streams.first)
        let stopped = expectation(description: "Recording stops while the connection stays open")
        capture.stop { result in
            XCTAssertEqual(try? result.get(), Array(repeating: Float(0.25), count: 160))
            stopped.fulfill()
        }
        wait(for: [stopped], timeout: 2)
        XCTAssertFalse(stream.stopped)
        for _ in 0..<10 { stream.receive(idle) }
        XCTAssertEqual(capture.sampleCount, 0)
        XCTAssertEqual(capture.level, 0)
        XCTAssertFalse(capture.isReady)
        rig.advance(299)
        stream.receive(idle)
        capture.refresh()
        resume(capture, feeding: stream, buffer: try firstBuffer(value: 0.5))
        XCTAssertEqual(rig.streams.count, 1, "A second dictation must not reconnect the microphone")
        let resumed = expectation(description: "Only new recording audio is returned")
        capture.stop { result in
            let samples = try? result.get()
            XCTAssertFalse(samples?.isEmpty ?? true)
            XCTAssertTrue(samples?.allSatisfy { $0 == 0.5 } ?? false)
            resumed.fulfill()
        }
        wait(for: [resumed], timeout: 2)
        rig.advance(299); stream.receive(idle); capture.refresh()
        XCTAssertFalse(stream.stopped, "Each recording renews the five-minute window")
        rig.advance(2); capture.refresh()
        eventually { stream.stopped }
        start(capture)
        XCTAssertEqual(rig.streams.count, 2)
    }

    func testIdleConnectionClosesOnRouteChangeOrInterruptionWithoutOpeningAnotherMicrophone() throws {
        for routeChange in [false, true] {
            let rig = try CaptureRig(buffer: firstBuffer()), capture = rig.capture()
            defer { capture.cancel() }
            start(capture)
            let stream = rig.streams[0], stopped = expectation(description: "Stopped")
            capture.stop { _ in stopped.fulfill() }
            wait(for: [stopped], timeout: 2)
            XCTAssertFalse(stream.stopped)
            if routeChange { rig.input = CaptureRig.builtIn; capture.refresh() }
            else { stream.interrupted("Bluetooth disconnected") }
            eventually { stream.stopped }
            XCTAssertEqual(rig.streams.count, 1)
            XCTAssertEqual(capture.sampleCount, 0)
            start(capture)
            XCTAssertEqual(rig.streams.count, 2)
        }
    }
    func testCancellationKeepsTheConnectionButShutdownReleasesIt() throws {
        let rig = try CaptureRig(buffer: firstBuffer()), capture = rig.capture()
        start(capture)
        let stream = rig.streams[0]
        capture.cancel(keepWarm: true)
        eventually { capture.isKeepingAlive }
        stream.receive(try firstBuffer(value: 0.75))
        XCTAssertEqual(capture.sampleCount, 0)
        resume(capture, feeding: stream, buffer: try firstBuffer(value: 0.5))
        XCTAssertEqual(rig.streams.count, 1)
        capture.cancel()
        eventually { stream.stopped }
        XCTAssertFalse(capture.isKeepingAlive)
        XCTAssertEqual(capture.sampleCount, 0)
    }

    private func resume(_ capture: AudioCapture, feeding stream: TestMicrophoneStream, buffer: CMSampleBuffer) {
        let resumed = expectation(description: "Fresh audio arrives on the warm connection")
        var done = false
        capture.start(uid: nil) { result in
            if case .failure(let error) = result { XCTFail(error.localizedDescription) }
            done = true; resumed.fulfill()
        }
        func feed() {
            guard !done else { return }
            stream.receive(buffer)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.02, execute: feed)
        }
        DispatchQueue.main.async(execute: feed)
        wait(for: [resumed], timeout: 2)
        done = true
    }
    func testFirstTenMillisecondsOfAudioAreReadyWithoutLosingSamples() throws {
        let capture = AudioCapture()
        capture.consume(try firstBuffer(), token: 0)
        let ready = expectation(description: "First audio is ready")
        // An expired deadline avoids waiting: real audio must take precedence.
        capture.waitForAudio(token: 0, deadline: 0) { result in
            if case .failure(let error) = result { XCTFail(error.localizedDescription) }
            ready.fulfill()
        }
        wait(for: [ready], timeout: 1)
        XCTAssertEqual(capture.sampleCount, 160)
        XCTAssertEqual(capture.level, 0.25, accuracy: 0.001)
    }

    func testNoAudioAndCancelledStartupNeverReportReady() throws {
        let capture = AudioCapture()
        let empty = expectation(description: "No audio times out")
        capture.waitForAudio(token: 0, deadline: 0) { result in
            if case .success = result { XCTFail("Empty capture is not ready") }
            empty.fulfill()
        }
        wait(for: [empty], timeout: 1)
        capture.cancel()
        capture.consume(try firstBuffer(), token: 0)
        let cancelled = expectation(description: "Cancelled startup stays cancelled")
        capture.waitForAudio(token: 0, deadline: .greatestFiniteMagnitude) { result in
            if case .success = result { XCTFail("Stale audio must not revive capture") }
            cancelled.fulfill()
        }
        wait(for: [cancelled], timeout: 1)
        XCTAssertEqual(capture.sampleCount, 0)
    }

    func testDigitalSilenceDoesNotProveTheMicrophoneIsReady() throws {
        let capture = AudioCapture()
        capture.consume(try firstBuffer(value: 0), token: 0)
        let empty = expectation(description: "Zero-filled capture is not ready")
        capture.waitForAudio(token: 0, deadline: 0) { result in
            if case .success = result { XCTFail("A stalled or muted microphone can keep sending zero-filled buffers") }
            empty.fulfill()
        }
        wait(for: [empty], timeout: 1)
    }

    func testDefaultInputChangeKeepsEarlierAudioAndRejectsOldStreamCallbacks() throws {
        let rig = try CaptureRig(buffer: firstBuffer())
        let capture = rig.capture()
        defer { capture.cancel() }
        start(capture)
        let old = try XCTUnwrap(rig.streams.first)
        rig.input = CaptureRig.builtIn
        capture.refresh()
        eventually { capture.activeInput == CaptureRig.builtIn && capture.isReady }
        XCTAssertEqual(capture.restartCount, 1)
        XCTAssertTrue(old.stopped)
        XCTAssertEqual(capture.sampleCount, 320)
        old.receive(try firstBuffer(value: 0.75))
        old.interrupted("Delayed failure from the old device")
        capture.refresh()
        let finished = expectation(description: "Both routes survive stop")
        capture.stop { result in
            switch result {
            case .success(let samples): XCTAssertEqual(samples, Array(repeating: Float(0.25), count: 320))
            case .failure(let error): XCTFail(error.localizedDescription)
            }
            finished.fulfill()
        }
        wait(for: [finished], timeout: 2)
    }

    func testExplicitInputStaysSelectedWhenSystemDefaultChanges() throws {
        let rig = try CaptureRig(buffer: firstBuffer())
        let capture = rig.capture()
        defer { capture.cancel() }
        start(capture, uid: CaptureRig.headset.uid)
        rig.input = CaptureRig.builtIn
        capture.refresh()
        let finished = expectation(description: "Refresh is processed before stop")
        capture.stop { _ in finished.fulfill() }
        wait(for: [finished], timeout: 2)
        XCTAssertEqual(rig.streams.count, 1)
    }

    func testInterruptedCaptureReopensWithoutDiscardingAudio() throws {
        let rig = try CaptureRig(buffer: firstBuffer())
        let capture = rig.capture()
        defer { capture.cancel() }
        start(capture)
        rig.streams[0].interrupted("Meeting changed the audio route")
        eventually { !capture.isReady }
        rig.advance(0.5)
        capture.refresh()
        eventually { capture.restartCount == 1 && capture.isReady }
        XCTAssertEqual(capture.sampleCount, 320)
    }

    func testMissingBuffersAndZeroFilledBuffersBothRecover() throws {
        for zeros in [false, true] {
            let rig = try CaptureRig(buffer: firstBuffer())
            let capture = rig.capture()
            defer { capture.cancel() }
            start(capture)
            rig.advance(3.1)
            if zeros { rig.streams[0].receive(try firstBuffer(value: 0)) }
            capture.refresh()
            eventually { !capture.isReady }
            rig.advance(0.5)
            capture.refresh()
            eventually { capture.restartCount == 1 && capture.isReady }
            XCTAssertEqual(capture.sampleCount, zeros ? 480 : 320)
        }
    }

    func testTemporaryLossOfDefaultInputUsesLatestDeviceWhenItReturns() throws {
        let rig = try CaptureRig(buffer: firstBuffer())
        let capture = rig.capture()
        defer { capture.cancel() }
        start(capture)
        rig.input = nil
        capture.refresh()
        eventually { !capture.isReady }
        rig.input = CaptureRig.builtIn
        rig.advance(0.5)
        capture.refresh()
        eventually { capture.activeInput == CaptureRig.builtIn && capture.isReady }
        XCTAssertEqual(capture.sampleCount, 320)
    }

    func testRecoveryHasDeadlineAndClosesTheMicrophone() throws {
        let rig = try CaptureRig(buffer: firstBuffer())
        let capture = rig.capture()
        defer { capture.cancel() }
        start(capture)
        rig.input = nil
        capture.refresh()
        eventually { !capture.isReady }
        let failed = expectation(description: "Unavailable input fails visibly")
        capture.onFailure = { _ in failed.fulfill() }
        rig.advance(12.1)
        capture.refresh()
        wait(for: [failed], timeout: 2)
        XCTAssertTrue(rig.streams.allSatisfy(\.stopped))
        XCTAssertFalse(capture.isReady)
    }

    func testHealthyAudioAtRecoveryDeadlineWinsOverTimeout() throws {
        let buffer = try firstBuffer()
        let rig = try CaptureRig(buffer: buffer)
        let capture = rig.capture()
        defer { capture.cancel() }
        start(capture)
        rig.streams[0].interrupted("Interrupted")
        eventually { !capture.isReady }
        rig.advance(10.9)
        capture.refresh()
        eventually { capture.isReady }
        rig.advance(0.5); rig.streams[1].receive(buffer)
        rig.advance(0.7); rig.streams[1].receive(buffer)
        capture.refresh()
        let finished = expectation(description: "Recovered audio can still be transcribed")
        capture.stop { result in
            switch result {
            case .success(let samples): XCTAssertEqual(samples.count, 640)
            case .failure(let error): XCTFail(error.localizedDescription)
            }
            finished.fulfill()
        }
        wait(for: [finished], timeout: 2)
    }

    func testCancelDuringRecoveryCannotRestartCapture() throws {
        let rig = try CaptureRig(buffer: firstBuffer())
        let capture = rig.capture()
        start(capture)
        let old = rig.streams[0]
        old.interrupted("Interrupted")
        eventually { !capture.isReady }
        capture.cancel()
        rig.advance(1)
        capture.refresh()
        old.receive(try firstBuffer())
        old.interrupted("Late interruption")
        let drained = expectation(description: "Cancelled callbacks drained")
        capture.stop { _ in drained.fulfill() }
        wait(for: [drained], timeout: 2)
        XCTAssertEqual(rig.streams.count, 1)
        XCTAssertEqual(capture.sampleCount, 0)
        XCTAssertFalse(capture.isReady)
    }

    func testStopFinishingAfterCancelDoesNotInvalidateTheNextRecording() throws {
        let rig = try CaptureRig(buffer: firstBuffer())
        let capture = rig.capture()
        defer { capture.cancel() }
        start(capture)
        let stopping = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0)
        // A stale stream must close instead of entering keepalive. Exercise the
        // cancellation race while that physical close is still in progress.
        rig.advance(3.1)
        rig.streams[0].onStop = { stopping.signal(); _ = release.wait(timeout: .now() + 2) }
        let stopped = expectation(description: "Old stop completes")
        capture.stop { _ in stopped.fulfill() }
        XCTAssertEqual(stopping.wait(timeout: .now() + 2), .success)
        capture.cancel()
        let started = expectation(description: "New recording survives old stop")
        capture.start(uid: nil) { result in
            if case .failure(let error) = result { XCTFail(error.localizedDescription) }
            started.fulfill()
        }
        release.signal()
        wait(for: [stopped, started], timeout: 2)
        XCTAssertTrue(capture.isReady)
    }

    private func start(_ capture: AudioCapture, uid: String? = nil) {
        let started = expectation(description: "Capture is receiving sound")
        capture.start(uid: uid) { result in
            if case .failure(let error) = result { XCTFail(error.localizedDescription) }
            started.fulfill()
        }
        wait(for: [started], timeout: 2)
    }

    private func eventually(_ condition: @escaping () -> Bool) {
        let condition = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in condition() }, object: nil)
        wait(for: [condition], timeout: 2)
    }

    private func firstBuffer(value: Float = 0.25) throws -> CMSampleBuffer {
        try sampleBuffer(Array(repeating: value, count: 160))
    }
    private func sampleBuffer(_ values: [Float]) throws -> CMSampleBuffer {
        let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: true)!
        let pcm = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(values.count))!
        pcm.frameLength = AVAudioFrameCount(values.count)
        for index in values.indices { pcm.floatChannelData![0][index] = values[index] }
        var description: CMAudioFormatDescription?
        XCTAssertEqual(CMAudioFormatDescriptionCreate(allocator: kCFAllocatorDefault, asbd: format.streamDescription,
            layoutSize: 0, layout: nil, magicCookieSize: 0, magicCookie: nil, extensions: nil, formatDescriptionOut: &description), noErr)
        var timing = CMSampleTimingInfo(duration: CMTime(value: 1, timescale: 16_000), presentationTimeStamp: .zero, decodeTimeStamp: .invalid)
        var buffer: CMSampleBuffer?
        XCTAssertEqual(CMSampleBufferCreate(allocator: kCFAllocatorDefault, dataBuffer: nil, dataReady: false,
            makeDataReadyCallback: nil, refcon: nil, formatDescription: description, sampleCount: values.count,
            sampleTimingEntryCount: 1, sampleTimingArray: &timing, sampleSizeEntryCount: 0, sampleSizeArray: nil, sampleBufferOut: &buffer), noErr)
        let result = try XCTUnwrap(buffer)
        XCTAssertEqual(CMSampleBufferSetDataBufferFromAudioBufferList(result, blockBufferAllocator: kCFAllocatorDefault,
            blockBufferMemoryAllocator: kCFAllocatorDefault, flags: 0, bufferList: pcm.audioBufferList), noErr)
        return result
    }

    func testFileResamplingPreservesDurationAndFiniteSamples() throws {
        for (rate, channels) in [(16_000.0, 1), (22_050.0, 1), (44_100.0, 2), (48_000.0, 2), (96_000.0, 1)] {
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("hey-talk-test-\(UUID()).caf")
            defer { try? FileManager.default.removeItem(at: url) }
            let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: rate, channels: AVAudioChannelCount(channels), interleaved: false)!
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(rate))!
            buffer.frameLength = buffer.frameCapacity
            for channel in 0..<channels {
                for i in 0..<Int(buffer.frameLength) { buffer.floatChannelData![channel][i] = Float(sin(Double(i) * 2 * .pi * 440 / rate) * 0.2) }
            }
            do {
                let file = try AVAudioFile(forWriting: url, settings: format.settings)
                try file.write(from: buffer)
            }
            let samples = try AudioCapture.readFile(url.path)
            XCTAssertEqual(samples.count, 16_000, accuracy: 2, "rate \(rate)")
            XCTAssertTrue(samples.allSatisfy(\.isFinite))
            XCTAssertGreaterThan(samples.map { abs($0) }.max() ?? 0, 0.1)
        }
    }
    func testLongDictationSplitsAtPausesAndKeepsTheFinalPartialWindow() {
        let rate = PCMFrame.rate
        var samples = Array(repeating: Float(0.5), count: 74 * rate + 1)
        let firstPause = 29 * rate + rate / 2
        let secondPause = firstPause + 80 + 29 * rate + rate / 2
        for pause in [firstPause, secondPause] {
            samples.replaceSubrange(pause..<pause+160, with: repeatElement(0, count: 160))
        }
        let first = AudioCapture.recognitionChunk(samples[...])
        let second = AudioCapture.recognitionChunk(samples[first.endIndex...])
        let tail = AudioCapture.recognitionChunk(samples[second.endIndex...])
        XCTAssertEqual(first.endIndex, firstPause + 80)
        XCTAssertEqual(second.endIndex, secondPause + 80)
        XCTAssertEqual(tail.endIndex, samples.endIndex)
        XCTAssertEqual(Array(first) + second + tail, samples)
    }

    func testEmptyAndOverlongAudioFailCleanly() throws {
        let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false)!
        for seconds in [0, 301] {
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("hey-talk-test-\(UUID()).caf")
            defer { try? FileManager.default.removeItem(at: url) }
            do {
                let file = try AVAudioFile(forWriting: url, settings: format.settings)
                if seconds > 0 {
                    let silence = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 16_000)!
                    silence.frameLength = 16_000
                    memset(silence.floatChannelData![0], 0, 16_000 * MemoryLayout<Float>.size)
                    for _ in 0..<seconds { try file.write(from: silence) }
                }
            }
            XCTAssertThrowsError(try AudioCapture.readFile(url.path))
            if seconds > 0 {
                var counts: [Int] = []
                try AudioCapture.readFileChunks(AVAudioFile(forReading: url)) { counts.append($0.count) }
                XCTAssertGreaterThan(counts.count, 1)
                XCTAssertEqual(counts.reduce(0, +), seconds * 16_000)
                XCTAssertLessThanOrEqual(counts.max()!, PCMFrame.maxSamples)
            }
        }
    }
}

private final class TestMicrophoneStream: MicrophoneStream {
    let receive: (CMSampleBuffer) -> Void
    let interrupted: (String) -> Void
    private let buffer: CMSampleBuffer
    private let lock = NSLock()
    private var didStop = false
    private var stopCallback: (() -> Void)?
    var onStop: (() -> Void)? {
        get { lock.lock(); defer { lock.unlock() }; return stopCallback }
        set { lock.lock(); stopCallback = newValue; lock.unlock() }
    }
    var stopped: Bool { lock.lock(); defer { lock.unlock() }; return didStop }
    init(buffer: CMSampleBuffer, receive: @escaping (CMSampleBuffer) -> Void, interrupted: @escaping (String) -> Void) {
        self.buffer = buffer; self.receive = receive; self.interrupted = interrupted
    }
    func start() throws { receive(buffer) }
    func stop() {
        lock.lock(); let callback = didStop ? nil : stopCallback; didStop = true; lock.unlock()
        callback?()
    }
}

private final class CaptureRig {
    static let headset = InputDevice(id: 10_001, uid: "test-headset", name: "Test headset")
    static let builtIn = InputDevice(id: 10_002, uid: "test-built-in", name: "Test built-in")
    static let headsetReplacement = InputDevice(id: 10_003, uid: "test-replacement", name: "Replacement headset")
    private let lock = NSLock()
    private let buffer: CMSampleBuffer
    private var selected: InputDevice? = headset
    private var opened: [TestMicrophoneStream] = []
    private var time: TimeInterval = 100
    var input: InputDevice? {
        get { lock.lock(); defer { lock.unlock() }; return selected }
        set { lock.lock(); selected = newValue; lock.unlock() }
    }
    var streams: [TestMicrophoneStream] { lock.lock(); defer { lock.unlock() }; return opened }
    init(buffer: CMSampleBuffer) throws { self.buffer = buffer }
    func advance(_ interval: TimeInterval) { lock.lock(); time += interval; lock.unlock() }
    func capture(fallback: @escaping (InputDevice) -> InputDevice? = { _ in nil }) -> AudioCapture {
        AudioCapture(resolveInput: { uid in
            if let uid, uid == Self.headset.uid { return Self.headset }
            guard let device = self.input else { throw TalkError("No system input") }
            return device
        }, fallbackInput: fallback, makeStream: { _, _, receive, interrupted in
            let stream = TestMicrophoneStream(buffer: self.buffer, receive: receive, interrupted: interrupted)
            self.lock.lock(); self.opened.append(stream); self.lock.unlock()
            return stream
        }, now: {
            self.lock.lock(); defer { self.lock.unlock() }; return self.time
        })
    }
}
