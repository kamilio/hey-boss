import XCTest
@testable import HeyTalk

final class SelectionCaptureTests: XCTestCase {
    @MainActor func testCaptureChoicesSurviveChatChangesAndStayInTheirOwnMode() throws {
        let suite = UUID().uuidString
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let capture = SelectionCapture(defaults: defaults)
        let statusItem = capture.menuItem()
        capture.setConversation(["selection_chat": "first"])
        XCTAssertTrue(capture.shouldMonitor)
        XCTAssertEqual(capture.menuItem(.dictation).state, .off)
        XCTAssertTrue(capture.validateMenuItem(statusItem))
        XCTAssertEqual(statusItem.state, .on)

        capture.setConversation(nil)
        XCTAssertFalse(capture.shouldMonitor, "Leaving a voice chat must not enable dictation capture")
        defaults.set(true, forKey: SelectionCapture.Mode.dictation.rawValue)
        let dictationItem = capture.menuItem(.dictation)
        XCTAssertTrue(NSApplication.shared.sendAction(try XCTUnwrap(dictationItem.action), to: dictationItem.target, from: dictationItem))
        capture.setConversation(["selection_chat": "first"])
        XCTAssertTrue(capture.shouldMonitor, "Turning off dictation capture must not mute chat capture")

        let chatItem = capture.menuItem(.conversation)
        XCTAssertTrue(NSApplication.shared.sendAction(try XCTUnwrap(chatItem.action), to: chatItem.target, from: chatItem))
        capture.setConversation([:]) // microphone paused
        capture.setConversation(["selection_chat": "first"])
        XCTAssertFalse(capture.shouldMonitor)
        capture.setConversation(nil)
        XCTAssertFalse(capture.shouldMonitor)
        capture.setConversation(["selection_chat": "second"])
        XCTAssertFalse(capture.shouldMonitor)

        defaults.set(true, forKey: SelectionCapture.Mode.dictation.rawValue)
        let relaunched = SelectionCapture(defaults: defaults)
        relaunched.setConversation(nil)
        XCTAssertTrue(relaunched.shouldMonitor)
        relaunched.setConversation([:])
        XCTAssertFalse(relaunched.shouldMonitor)
        relaunched.setConversation(["selection_chat": "second"])
        XCTAssertFalse(relaunched.shouldMonitor)
        XCTAssertEqual(relaunched.menuItem(.conversation).state, .off)
        XCTAssertEqual(relaunched.menuItem(.dictation).state, .on)
    }

    func testBrowserAddressFieldsAreDistinctFromOtherSearchFields() {
        XCTAssertTrue(SelectionCapture.isAddressField(identifier: "WEB_BROWSER_ADDRESS_AND_SEARCH_FIELD", domID: "", description: ""))
        XCTAssertTrue(SelectionCapture.isAddressField(identifier: "", domID: "view_1012", description: "Address and search bar"))
        XCTAssertTrue(SelectionCapture.isAddressField(identifier: "", domID: "urlbar-input", description: ""))
        XCTAssertFalse(SelectionCapture.isAddressField(identifier: "", domID: "", description: "Search bookmarks"))
    }

    func testFullSelectionAndSourceAreLiteralInStandardNotice() {
        let text = "    func foo() {\n\n```\n![image](https://invalid.test/x)"
        let notice = SelectionCapture.notice(app: "cmux", detail: "10:01 · devbox", text: text)
        XCTAssertEqual(notice["command"] as? String, "selection_notice")
        XCTAssertEqual(notice["selection_text"] as? String, text)
        XCTAssertEqual(notice["selection_source"] as? String, "cmux · 10:01 · devbox")
        XCTAssertEqual(notice["title"] as? String, "cmux")
        XCTAssertEqual(notice["question"] as? String, "````\n10:01 · devbox\n\n" + text + "\n````")
        XCTAssertEqual(notice["sync"] as? Bool, false)
    }
}
