import XCTest
@testable import HeyTalk

final class DeckTests: XCTestCase {
    func testConversationIndicatorsFollowDesktopEvenWithoutADeckPress() {
        let service = TalkService(headless: true)
        func request(_ action: String? = nil) -> [String: Any] {
            var command = ["command": "deck"]
            command["action"] = action
            var response: [String: Any] = [:]
            service.command(command) { response = $0 }
            return response
        }
        XCTAssertEqual(request()["conversation"] as? String, "Ready")
        XCTAssertEqual(request("mute")["ok"] as? Bool, false)
        service.applyConversation(["active": true, "phase": "listening", "message": "Listening paused", "listeningPaused": true])
        XCTAssertEqual(request()["mute"] as? String, "Muted")
        XCTAssertEqual(request()["conversation"] as? String, "Muted")
        service.applyConversation(nil)
        XCTAssertEqual(request()["conversation"] as? String, "Ready")
        XCTAssertEqual(request("unknown")["ok"] as? Bool, false)
    }
}
