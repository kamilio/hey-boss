import XCTest
@testable import HeyTalk

final class CatchUpTests: XCTestCase {
    func testStreamEmitsCompleteSentencesBeforeCompletionWithoutSplittingDecimals() throws {
        var stream = RecapStream()
        var sentences = [String]()
        func event(_ type: String, _ delta: String = "") throws -> Bool {
            let data = try JSONSerialization.data(withJSONObject: ["type":type, "delta":delta])
            return try stream.receive(String(decoding: data, as: UTF8.self)) { sentences.append($0) }
        }
        _ = try event("response.reasoning_summary_text.delta", "Ignore this.")
        _ = try event("response.output_text.delta", "Revenue grew 3.")
        XCTAssertTrue(sentences.isEmpty)
        _ = try event("response.output_text.delta", "5 percent. Jo")
        XCTAssertEqual(sentences, ["Revenue grew 3.5 percent."])
        _ = try event("response.output_text.delta", " will follow up")
        XCTAssertTrue(try event("response.completed"))
        XCTAssertEqual(sentences, ["Revenue grew 3.5 percent.", "Jo will follow up"])
    }

    func testIncompleteResponseDoesNotSpeakAnUnfinishedThought() throws {
        var stream = RecapStream()
        var spoken = [String]()
        _ = try stream.receive(#"{"type":"response.output_text.delta","delta":"It passed. But the"}"#) { spoken.append($0) }
        XCTAssertThrowsError(try stream.receive(#"{"type":"response.incomplete"}"#) { spoken.append($0) })
        XCTAssertEqual(spoken, ["It passed."])
    }

    func testT3ReadsOnlyAssistantTextAfterLastHumanMessage() throws {
        func row(_ type: String, _ text: String, _ author: String = "agent") -> [String: Any] {
            ["item": ["type": type, "text": text, "createdBy": author]]
        }
        let items = [row("user_message","first","user"), row("assistant_message","old"), row("user_message","latest","user"),
                     row("assistant_message","Checking."), row("reasoning","hidden"), row("command_execution","log"),
                     row("user_message","internal event"), row("assistant_message","Done.")]
        var replies = [String]()
        XCTAssertNil(try T3Server.catchUpText(Array(items.suffix(2)), newer: &replies))
        let passage = try XCTUnwrap(T3Server.catchUpText(Array(items.dropLast(2)), newer: &replies))
        XCTAssertEqual(passage.lastMessage, "latest")
        XCTAssertEqual(passage.replies, "Checking.\n\nDone.")
        replies = []
        XCTAssertThrowsError(try T3Server.catchUpText(Array(items.prefix(3)), newer: &replies))
    }

    func testChannelPagesBackToMeAndNamesSpeakers() async throws {
        var calls = [String]()
        let result = try await SlackTools.catchUp(URL(string: "https://app.slack.com/client/T123/C456")!) { method, p in
            calls.append(method)
            switch method {
            case "auth.test": return ["team_id":"T123","user_id":"ME"]
            case "users.info": return ["user":["profile":["display_name":"Jo"]]]
            default:
                if p["cursor"] == nil { return ["messages":[["ts":"3","user":"U2","text":"Done."]],"response_metadata":["next_cursor":"next"]] }
                return ["messages":[["ts":"2","user":"U2","text":"On it."],["ts":"1","user":"ME","text":"Question"]],"has_more":true]
            }
        }
        XCTAssertEqual(result.lastMessage,"Question")
        XCTAssertEqual(result.replies,"Jo: On it.\nJo: Done.")
        XCTAssertEqual(calls,["auth.test","conversations.history","conversations.history","users.info"])
    }
    func testThreadCanHaveALaterOwnReplyOnNextPage() async throws {
        let url = URL(string: "https://app.slack.com/client/T123/C456/thread/C456-1234567890.123456")!
        let text = try await SlackTools.catchUp(url) { method,p in
            if method == "auth.test" { return ["team_id":"T123","user_id":"ME"] }
            XCTAssertEqual(method,"conversations.replies"); XCTAssertEqual(p["ts"],"1234567890.123456")
            if p["cursor"] == nil { return ["messages":[["ts":"1","user":"ME"],["ts":"2","text":"old"]],"response_metadata":["next_cursor":"next"]] }
            return ["messages":[["ts":"3","user":"ME"],["ts":"4","username":"Build","text":"Passed."]]]
        }
        XCTAssertEqual(text.replies,"Build: Passed.")
    }
}
