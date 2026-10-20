import XCTest
import TalkCore
@testable import HeyTalk

final class InferenceVocabularyTests: XCTestCase {
    func testMissingVocabularyFailsBeforeStartingNativeInference() throws {
        let missing = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).path
        XCTAssertThrowsError(try runInferenceWorker(model: missing, library: missing, vocabulary: missing)) {
            XCTAssertEqual($0.localizedDescription, "Vocabulary files are missing. Run the hey-talk installer.")
        }
    }

    func testNamesAndOrdinarySpeechThroughPackagedWorker() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let appPath = environment["HEY_TALK_TEST_APP"] else {
            throw XCTSkip("Run ./test-vocabulary.sh after building the app")
        }
        let app = URL(fileURLWithPath: appPath)
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("hey-talk-vocabulary-\(UUID())")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let vocabulary = Vocabulary(file: directory.appendingPathComponent("vocabulary.md"),
                                    defaults: app.appendingPathComponent("Contents/Resources/vocabulary.md"))
        let original = try vocabulary.command([])

        let worker = Process(), input = Pipe(), output = Pipe()
        worker.executableURL = app.appendingPathComponent("Contents/Helpers/hey-talk-inference")
        worker.arguments = ["--worker", ModelPaths.directory.path,
            app.appendingPathComponent("Contents/Frameworks/libsherpa-onnx-c-api.dylib").path,
            vocabulary.file.path]
        worker.standardInput = input; worker.standardOutput = output
        try Wire.prepare(input.fileHandleForWriting.fileDescriptor)
        try Wire.prepare(output.fileHandleForReading.fileDescriptor)
        try worker.run()
        defer {
            try? input.fileHandleForWriting.close()
            if worker.isRunning { worker.terminate() }
            worker.waitUntilExit()
        }
        let ready = try JSONSerialization.jsonObject(with: Wire.line(from: output.fileHandleForReading.fileDescriptor, timeout: 60)) as? [String: Any]
        XCTAssertEqual(ready?["ready"] as? Bool, true, "\(ready ?? [:])")
        guard ready?["ready"] as? Bool == true else { return }

        func transcribe(_ samples: [Float]) throws -> String {
            let start = ProcessInfo.processInfo.systemUptime
            try Wire.write(PCMFrame.encode(samples), to: input.fileHandleForWriting.fileDescriptor)
            let response = try JSONSerialization.jsonObject(with: Wire.line(from: output.fileHandleForReading.fileDescriptor, timeout: 30)) as? [String: Any]
            let text = try XCTUnwrap(response?["text"] as? String)
            let seconds = ProcessInfo.processInfo.systemUptime - start
            print("Vocabulary: \(String(format: "%.3f", seconds))s / \(String(format: "%.2f", Double(samples.count) / 16_000))s audio: \(text)")
            XCTAssertLessThan(seconds, max(2, Double(samples.count) / 16_000))
            return text
        }

        let cases: [(String, [String])] = [
            ("Kamil uses hey boss to work on Poe.", ["Kamil", "hey boss", "Poe"]),
            ("Vercel runs behind Cloudflare. Claude uses a MacBook.", ["Cloudflare", "MacBook"]),
            ("The cloud is gray. A camel walks past the boss. I wrote a poem.", ["cloud", "camel", "boss", "poem"]),
        ]
        for (index, entry) in cases.enumerated() {
            let audio = directory.appendingPathComponent("\(index).aiff")
            let speech = Process()
            speech.executableURL = URL(fileURLWithPath: "/usr/bin/say")
            speech.arguments = ["-v", "Samantha", "-o", audio.path, entry.0]
            try speech.run(); speech.waitUntilExit()
            XCTAssertEqual(speech.terminationStatus, 0)
            let samples = try AudioCapture.readFile(audio.path)
            let text = try transcribe(samples).replacingOccurrences(of: "-", with: " ")
            for word in entry.1 { XCTAssertTrue(text.contains(word), "Missing \(word): \(text)") }
            if index == 0 {
                _ = try vocabulary.command(["remove"] + original)
                let withoutHints = try transcribe(samples)
                XCTAssertFalse(withoutHints.contains("Kamil"), "Fixture must demonstrate a vocabulary correction")
                _ = try vocabulary.command(["add", "Kamil"])
                XCTAssertTrue(try transcribe(samples).contains("Kamil"), "Adding a term must affect the next decode without a worker restart")
                _ = try vocabulary.command(["remove", "Kamil"])
                XCTAssertEqual(try transcribe(samples), withoutHints, "Removed terms must not linger in the warm worker")
                _ = try vocabulary.command(["add"] + original)
            }
        }
        XCTAssertTrue(try transcribe([Float](repeating: 0, count: 16_000)).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    }
}
