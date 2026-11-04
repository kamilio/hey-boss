import Foundation
import XCTest
@testable import HeyTalk

final class CorrectionEvalTests: XCTestCase {
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
