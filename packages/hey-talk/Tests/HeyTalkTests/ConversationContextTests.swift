import XCTest
@testable import HeyTalk

final class ConversationContextTests: XCTestCase {
    func testSlackHistoryUsesOnlyTheOpenConversation() {
        for channel in ["C12", "D34", "G56"] {
            let url = "https://app.slack.com/client/E123/" + channel
            XCTAssertEqual(ConversationContext.slackURL(url), url)
        }
        XCTAssertEqual(ConversationContext.slackURL("https://app.slack.com/client/T12/C34/thread/C34-1234567890.123456"),
                       "https://app.slack.com/client/T12/C34/thread/C34-1234567890.123456")
        XCTAssertNil(ConversationContext.slackURL("https://app.slack.com/client/T12/activity-inbox"))
        XCTAssertNil(ConversationContext.slackURL("https://example.org/client/T12/C34"))
        XCTAssertNil(ConversationContext.slackURL("https://app.slack.com/client/T12/C34/thread/D66-1234567890.123456"))
    }
    func testChannelPagesAndThreadRepliesAreMergedOnceInTimeOrder() async throws {
        let text = try await SlackTools.history(URL(string: "https://app.slack.com/client/T1/D2")!) { method, params in
            switch method {
            case "auth.test": return ["team_id": "T1"]
            case "conversations.history" where params["cursor"] == nil:
                return ["messages": [["ts": "3", "text": "newest"]], "response_metadata": ["next_cursor": "older"]]
            case "conversations.history":
                XCTAssertEqual(params["cursor"], "older")
                return ["messages": [["ts": "1", "text": "parent", "reply_count": 1]]]
            case "conversations.replies":
                XCTAssertEqual(params["ts"], "1")
                return ["messages": [["ts": "1", "text": "parent"], ["ts": "2", "thread_ts": "1", "text": "reply"]]]
            default: XCTFail(method); return [:]
            }
        }
        XCTAssertEqual(text.components(separatedBy: "parent").count, 2)
        XCTAssertTrue(text.range(of: "parent")!.lowerBound < text.range(of: "reply")!.lowerBound)
        XCTAssertTrue(text.range(of: "reply")!.lowerBound < text.range(of: "newest")!.lowerBound)
    }

}
