import Foundation
import XCTest
@testable import HeyTalk

final class CorrectionEvalTests: XCTestCase {
    func testLiveT3NavigationAndCurrentThreadActions() async throws {
        guard ProcessInfo.processInfo.environment["HEY_TALK_EVAL"] == "1" else { throw XCTSkip("Opt-in live Luna eval") }
        let correction = Correction()
        for (speech, expected): (String, [CorrectionAction]) in [
            ("Go to Publish Artifact conversation.", [.listThreads, .switchThread(8)]),
            ("Go to chat three.", [.switchThread(3)]),
            ("Let's settle this thread.", [.settleThread]),
            ("Let's pin this thread.", [.pinThread(true)]),
            ("Unpin the chat.", [.pinThread(false)]),
            ("Stop pinning this chat.", [.pinThread(false)])
        ] {
            var actions: [CorrectionAction] = []
            let _: CorrectionAction = try await withCheckedThrowingContinuation { continuation in
                correction.correct(speech, vocabulary: [], app: { ["name": "T3 Code", "bundle_id": "com.t3tools.t3code", "thread": "Voice tools"] }, package: .t3, perform: { action, next in
                    actions.append(action)
                    if action == .listThreads {
                        next(#"{"current_thread":"Voice tools","threads":[{"position":1,"title":"Voice tools","current":true},{"position":8,"title":"Publish Artifacts Publicly","current":false}]}"#)
                    } else { next("done") }
                }) { continuation.resume(with: $0) }
            }
            XCTAssertEqual(actions, expected, speech)
        }
        for speech in ["Add a button that pins the current thread.", "How do I settle a thread?"] {
            let action: CorrectionAction = try await withCheckedThrowingContinuation { continuation in
                correction.correct(speech, vocabulary: [], app: { ["name": "T3 Code"] }, package: .t3) { continuation.resume(with: $0) }
            }
            guard case .paste = action else { XCTFail("Unexpected action for \(speech)"); continue }
        }
    }

    func testLiveSequentialTools() async throws {
        guard ProcessInfo.processInfo.environment["HEY_TALK_EVAL"] == "1" else { throw XCTSkip("Opt-in live Luna eval") }
        let correction = Correction()
        for (speech, expected): (String, [CorrectionAction]) in [
            ("Clear this field, then paste the words Check publishing.", [.clearInput, .paste("check publishing")]),
            ("Check publishing.", [.paste("check publishing")])
        ] {
            var actions: [CorrectionAction] = []
            let _: CorrectionAction = try await withCheckedThrowingContinuation { continuation in
                correction.correct(speech, vocabulary: [], canClear: true, perform: { action, next in
                    actions.append(action)
                    next(action == .clearInput ? "cleared" : "pasted")
                }) { continuation.resume(with: $0) }
            }
            let normalized = actions.map { action -> CorrectionAction in
                guard case .paste(let text) = action else { return action }
                return .paste(text.trimmingCharacters(in: .punctuationCharacters).lowercased())
            }
            XCTAssertEqual(normalized, expected)
        }
    }

    func testLiveNewThreadTool() async throws {
        guard ProcessInfo.processInfo.environment["HEY_TALK_EVAL"] == "1" else { throw XCTSkip("Opt-in live Luna eval") }
        let correction = Correction()
        func run(_ speech: String, context: String = "") async throws -> CorrectionAction {
            try await withCheckedThrowingContinuation { continuation in
                correction.correct(speech, vocabulary: ["hey-boss"], appContext: context, package: .t3, canClear: true) { continuation.resume(with: $0) }
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
                correction.correct(transcript, vocabulary: vocabulary, appContext: context, package: threadActions ? .t3 : nil) { continuation.resume(with: $0) }
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
