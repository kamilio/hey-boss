import XCTest
import AVFoundation
import TalkCore
@testable import HeyTalk

final class SpeechInferenceTests: XCTestCase {
    func testFirstAudioDoesNotDependOnLoadingWordAlignment() throws {
        guard let path = ProcessInfo.processInfo.environment["HEY_TALK_TEST_APP"] else { throw XCTSkip("Run test-speech.sh after building the app") }
        let app = URL(fileURLWithPath: path), worker = Process(), input = Pipe(), output = Pipe()
        worker.executableURL = app.appendingPathComponent("Contents/Helpers/hey-talk-inference")
        worker.arguments = ["--speech-worker", SpeechModel.directory.path, app.appendingPathComponent("Contents/Frameworks/libsherpa-onnx-c-api.dylib").path, "/nonexistent-word-alignment-model"]
        worker.standardInput = input; worker.standardOutput = output
        try Wire.prepare(input.fileHandleForWriting.fileDescriptor); try Wire.prepare(output.fileHandleForReading.fileDescriptor)
        try worker.run()
        defer { if worker.isRunning { worker.terminate() }; worker.waitUntilExit() }
        let ready = try JSONSerialization.jsonObject(with: Wire.line(from: output.fileHandleForReading.fileDescriptor, timeout: 30)) as? [String: Any]
        XCTAssertEqual(ready?["ready"] as? Bool, true, "Alignment must not block speech startup")
        guard ready?["ready"] as? Bool == true else { return }
        try Wire.write(Wire.json(["text": "Hello there."]), to: input.fileHandleForWriting.fileDescriptor)
        let header = try JSONSerialization.jsonObject(with: Wire.line(from: output.fileHandleForReading.fileDescriptor, timeout: 30)) as? [String: Any]
        let count = try XCTUnwrap(header?["count"] as? Int)
        XCTAssertGreaterThan(count, 1000)
        _ = try Wire.readExactly(count * 4, from: output.fileHandleForReading.fileDescriptor)
        let alignment = try JSONSerialization.jsonObject(with: Wire.line(from: output.fileHandleForReading.fileDescriptor, timeout: 30)) as? [String: Any]
        XCTAssertEqual(alignment?["tokens"] as? [String], [], "Missing alignment uses the existing phrase highlight")
    }
    func testPackagedSupertonicProducesAudibleRecognizableSpeech() throws {
        guard let path = ProcessInfo.processInfo.environment["HEY_TALK_TEST_APP"] else {
            throw XCTSkip("Run test-speech.sh after building the app")
        }
        let app = URL(fileURLWithPath: path)
        let worker = Process(), input = Pipe(), output = Pipe()
        worker.executableURL = app.appendingPathComponent("Contents/Helpers/hey-talk-inference")
        worker.arguments = ["--speech-worker", SpeechModel.directory.path, app.appendingPathComponent("Contents/Frameworks/libsherpa-onnx-c-api.dylib").path, ModelPaths.directory.path]
        worker.standardInput = input; worker.standardOutput = output
        try Wire.prepare(input.fileHandleForWriting.fileDescriptor); try Wire.prepare(output.fileHandleForReading.fileDescriptor)
        let cold = ProcessInfo.processInfo.systemUptime
        try worker.run()
        defer { if worker.isRunning { worker.terminate() }; worker.waitUntilExit() }
        let ready = try JSONSerialization.jsonObject(with: Wire.line(from: output.fileHandleForReading.fileDescriptor, timeout: 60)) as? [String: Any]
        XCTAssertEqual(ready?["ready"] as? Bool, true, "\(ready ?? [:])")
        guard ready?["ready"] as? Bool == true else { return }
        print("Supertonic model load: \(Int((ProcessInfo.processInfo.systemUptime - cold) * 1000))ms")
        var first: [Float] = []
        var alignedSentences = 0
        let sentences = ["Hello there. This paragraph can now be read aloud.", "The quick brown fox jumps over the lazy dog.", "Speech stays on this computer."]
        var generated = 0
        // Alignment warms independently and can take longer under CPU pressure.
        // Keep proving audio works while waiting, with a bounded readiness check.
        let alignmentDeadline = ProcessInfo.processInfo.systemUptime + 30
        while generated < 6 || (alignedSentences == 0 && ProcessInfo.processInfo.systemUptime < alignmentDeadline) {
            let text = sentences[generated % sentences.count]
            generated += 1
            let start = ProcessInfo.processInfo.systemUptime
            try Wire.write(Wire.json(["text": text]), to: input.fileHandleForWriting.fileDescriptor)
            let response = try JSONSerialization.jsonObject(with: Wire.line(from: output.fileHandleForReading.fileDescriptor, timeout: 30)) as? [String: Any]
            let count = try XCTUnwrap(response?["count"] as? Int, "\(response ?? [:])")
            XCTAssertEqual(response?["rate"] as? Int, 44_100)
            guard count > 0, count < 44_100 * 120 else { XCTFail("Invalid audio length"); return }
            let data = try Wire.readExactly(count * 4, from: output.fileHandleForReading.fileDescriptor)
            let samples = data.withUnsafeBytes { bytes in stride(from: 0, to: data.count, by: 4).map { bytes.loadUnaligned(fromByteOffset: $0, as: Float.self) } }
            XCTAssertTrue(samples.allSatisfy(\.isFinite))
            XCTAssertGreaterThan(samples.map { abs($0) }.max() ?? 0, 0.01)
            print("Supertonic generation: \(Int((ProcessInfo.processInfo.systemUptime - start) * 1000))ms for \(String(format: "%.2f", Double(count) / 44_100))s audio")
            let alignment = try JSONSerialization.jsonObject(with: Wire.line(from: output.fileHandleForReading.fileDescriptor)) as? [String: Any]
            let words = SpeechTiming.align(text: text, tokens: alignment?["tokens"] as? [String] ?? [], times: alignment?["timestamps"] as? [Double] ?? [], duration: Double(count) / 44_100)
            if !words.isEmpty {
                alignedSentences += 1
                if text.hasPrefix("Hello") { XCTAssertEqual(words.count, 9, "\(alignment ?? [:])") }
            }
            print("Word timing ready: \(Int((ProcessInfo.processInfo.systemUptime - start) * 1000))ms, \(words.count) aligned words")
            if first.isEmpty { first = samples }
        }
        XCTAssertGreaterThan(alignedSentences, 0, "Word timing must become available after background initialization")
        // An independent speech recognizer checks that the waveform says the
        // requested words, rather than merely containing nonzero audio.
        let temporary = FileManager.default.temporaryDirectory.appendingPathComponent("hey-talk-speech-\(UUID()).wav")
        defer { try? FileManager.default.removeItem(at: temporary) }
        let format = try XCTUnwrap(AVAudioFormat(standardFormatWithSampleRate: 44_100, channels: 1))
        let buffer = try XCTUnwrap(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(first.count)))
        buffer.frameLength = buffer.frameCapacity
        first.withUnsafeBufferPointer { buffer.floatChannelData![0].update(from: $0.baseAddress!, count: $0.count) }
        do {
            let file = try AVAudioFile(forWriting: temporary, settings: format.settings)
            try file.write(from: buffer)
        }
        let samples = try AudioCapture.readFile(temporary.path)
        let recognizer = Process(), ri = Pipe(), ro = Pipe()
        recognizer.executableURL = worker.executableURL
        recognizer.arguments = ["--worker", ModelPaths.directory.path, worker.arguments![2], app.appendingPathComponent("Contents/Resources/vocabulary.md").path]
        recognizer.standardInput = ri; recognizer.standardOutput = ro
        try Wire.prepare(ri.fileHandleForWriting.fileDescriptor); try Wire.prepare(ro.fileHandleForReading.fileDescriptor)
        try recognizer.run()
        defer { if recognizer.isRunning { recognizer.terminate() }; recognizer.waitUntilExit() }
        _ = try Wire.line(from: ro.fileHandleForReading.fileDescriptor, timeout: 60)
        try Wire.write(PCMFrame.encode(samples), to: ri.fileHandleForWriting.fileDescriptor)
        let result = try JSONSerialization.jsonObject(with: Wire.line(from: ro.fileHandleForReading.fileDescriptor)) as? [String: Any]
        let heard = try XCTUnwrap(result?["text"] as? String).lowercased()
        print("Supertonic recognized: \(heard)")
        for word in ["hello", "paragraph", "read", "aloud"] { XCTAssertTrue(heard.contains(word), heard) }
    }
}
