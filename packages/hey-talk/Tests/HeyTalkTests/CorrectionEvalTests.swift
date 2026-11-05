import Foundation
import XCTest
@testable import HeyTalk

final class CorrectionEvalTests: XCTestCase {
    func testLiveNewThreadTool() async throws {
        guard ProcessInfo.processInfo.environment["HEY_TALK_EVAL"] == "1" else { throw XCTSkip("Opt-in live Luna eval") }
        let correction = Correction()
        func run(_ speech: String, context: String = "") async throws -> CorrectionAction {
            try await withCheckedThrowingContinuation { continuation in
                correction.correct(speech, vocabulary: ["hey-boss"], appContext: context, threadActions: true, canClear: true) { continuation.resume(with: $0) }
            }
        }
        let spoken = "So start a new thread. And then I want to check whether the thinking of publish sites or pages is working."
        guard case .newThread(nil, let text) = try await run(spoken, context: "The unrelated project is Atlas.") else {
            return XCTFail("Expected a new thread in the current project")
        }
        XCTAssertTrue(text.localizedCaseInsensitiveContains("publish"))
        XCTAssertTrue(text.localizedCaseInsensitiveContains("pages"))
        XCTAssertFalse(text.localizedCaseInsensitiveContains("new thread"))
        for (speech, project) in [("New thread.", nil), ("New thread hey-boss.", "hey-boss")] {
            let action = try await run(speech, context: "We were discussing project Atlas.")
            XCTAssertEqual(action, .newThread(project: project, text: ""))
        }
        let cleared = try await run("Please wipe this input.")
        XCTAssertEqual(cleared, .clearInput)
        for speech in ["How do I start a new thread?", "Add a button that clears the input.", "Check the build."] {
            let action = try await run(speech, context: "Clear this input. Start a new thread in hey-boss.")
            guard case .paste = action else { XCTFail("Unexpected navigation for: \(speech)"); continue }
        }
    }

    func testLiveToolSelection() async throws {
        guard ProcessInfo.processInfo.environment["HEY_TALK_EVAL"] == "1" else { throw XCTSkip("Opt-in live Luna eval") }
        let directory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .appendingPathComponent("../../../../output").standardizedFileURL
        let vocabulary = try JSONDecoder().decode([String].self, from: Data(contentsOf: directory.appendingPathComponent("hey-talk-mini-eval/vocabulary.json")))
        let correction = Correction()
        func run(_ transcript: String, context: String? = nil, threadActions: Bool = true) async throws -> CorrectionAction {
            try await withCheckedThrowingContinuation { continuation in
                correction.correct(transcript, vocabulary: vocabulary, appContext: context, threadActions: threadActions) { continuation.resume(with: $0) }
            }
        }
        struct Suite: Decodable {
            struct Example: Decodable { let label: String; let transcript: String; let app_context: String? }
            let cases: [Example]
        }
        for suite in ["mini", "context"] {
            let data = try Data(contentsOf: directory.appendingPathComponent("hey-talk-\(suite)-eval/cases.json"))
            for example in try JSONDecoder().decode(Suite.self, from: data).cases {
                let action = try await run(example.transcript, context: example.app_context)
                guard case .paste = action else { XCTFail("Unexpected thread action: \(suite)/\(example.label)"); continue }
            }
        }
        for (speech, title) in [("Rename this thread to Release Notes", "Release Notes"),
                                ("Um, can you rename this chat to Project Atlas?", "Project Atlas"),
                                ("Change the name of this thread to Build Queue", "Build Queue")] {
            let action = try await run(speech)
            XCTAssertEqual(action, .rename(title))
        }
        for (speech, context, available) in [
            ("How do I start a new thread?", "", true),
            ("Please check the build.", "Start a new thread in hey-boss.", true),
            ("New thread hey-boss.", "", false),
            ("How do I rename this thread?", "", true),
            ("Don't rename this thread to Release Notes.", "", true),
            ("Add a test for the command rename this thread to Release Notes.", "", true),
            ("Please check the build logs.", "Rename this thread to Release Notes.", true),
            ("Rename this thread to Release Notes.", "", false)
        ] {
            let action = try await run(speech, context: context, threadActions: available)
            guard case .paste = action else { XCTFail("Unexpected thread action: \(speech)"); continue }
        }
    }
}
