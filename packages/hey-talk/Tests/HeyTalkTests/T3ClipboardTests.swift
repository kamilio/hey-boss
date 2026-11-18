import AppKit
import XCTest
import TalkCore
@testable import HeyTalk

final class T3ClipboardTests: XCTestCase {
    @MainActor
    func testLivePasteRestoresClipboardAndSkipsHistory() async throws {
        guard ProcessInfo.processInfo.environment["HEY_TALK_T3_CLIPBOARD_TEST"] == "1" else { throw XCTSkip("Requires a focused empty T3 scratch draft, Hey Talk, and Hey Clipboard") }
        let app = try XCTUnwrap(NSRunningApplication.runningApplications(withBundleIdentifier: "com.t3tools.t3code").first)
        app.activate()
        try await Task.sleep(nanoseconds: 200_000_000)
        let field = try XCTUnwrap(FocusedInput.read(app: app, forAuto: false))
        guard field.document?.hasPrefix("t3code://app/#/draft/") == true,
              SpeechAX.string(field.element, "AXValue").trimmingCharacters(in: .newlines).isEmpty else {
            return XCTFail("Open an empty scratch draft; this test leaves unsent fixture text there for inspection")
        }
        let clipboardFile = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/hey-clipboard/history.json")
        let historyBefore = try Data(contentsOf: clipboardFile)
        func snapshot() -> [[String: Data]] {
            (NSPasteboard.general.pasteboardItems ?? []).map { item in
                Dictionary(uniqueKeysWithValues: item.types.compactMap { type in item.data(forType: type).map { (type.rawValue, $0) } })
            }
        }
        let originalClipboard = snapshot()
        let text = "Hey Talk clipboard verification 🌍\nSecond line — never submit"
        let done = expectation(description: "pasted in draft")
        // The installed signed app owns event-posting permission; xctest does not.
        Paste.send(text, shouldPressEnter: { false }, check: { field.isFocused() ? nil : .focusChanged }, isCurrent: { true }, post: { key, flags in
            XCTAssertEqual(key, 9); XCTAssertEqual(flags, .maskCommand)
            return (try? ControlSocket.request(["command": "paste_clipboard", "pid": app.processIdentifier,
                                                "change_count": NSPasteboard.general.changeCount])["ok"] as? Bool) == true
        }, afterPaste: { DispatchQueue.main.asyncAfter(deadline: .now() + 0.5, execute: $0) }) { delivery, reason in
            XCTAssertEqual(delivery, "pasted"); XCTAssertNil(reason); done.fulfill()
        }
        await fulfillment(of: [done], timeout: 5)
        let fresh = try XCTUnwrap(FocusedInput.read(app: app, forAuto: false))
        XCTAssertEqual(SpeechAX.string(fresh.element, "AXValue").trimmingCharacters(in: .newlines), text)
        XCTAssertTrue(snapshot() == originalClipboard, "Previous clipboard contents must be restored")
        try await Task.sleep(nanoseconds: 500_000_000)
        XCTAssertTrue(try Data(contentsOf: clipboardFile) == historyBefore, "Temporary dictations must not change Hey Clipboard history")
    }
}
