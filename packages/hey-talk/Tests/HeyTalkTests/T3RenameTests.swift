import AppKit
import XCTest
@testable import HeyTalk

final class T3RenameTests: XCTestCase {
    @MainActor
    func testLiveRenameKeepsTheComposerDraft() async throws {
        guard ProcessInfo.processInfo.environment["HEY_TALK_T3_TEST"] == "1" else { throw XCTSkip("Requires a focused T3 thread") }
        let field = try XCTUnwrap(FocusedInput.current(bundleIDs: ["com.t3tools.t3code"]))
        let label = try XCTUnwrap(field.composerLabel)
        XCTAssertTrue(label.hasPrefix("Message "))
        let title = String(label.dropFirst("Message ".count))
        let draft = SpeechAX.string(field.element, "AXValue")
        let done = expectation(description: "title confirmed")
        Paste.deliver(.rename(title), target: field.app, originalField: field, shouldPressEnter: { false }, returnToField: nil,
                      targetStillFocused: field.isFocused, onWait: nil, isCurrent: { true }) { delivery, reason in
            XCTAssertEqual(delivery, "renamed"); XCTAssertNil(reason)
            done.fulfill()
        }
        await fulfillment(of: [done], timeout: 5)
        try await Task.sleep(nanoseconds: 150_000_000)
        XCTAssertNil(field.focusedThreadTitle())
        XCTAssertEqual(SpeechAX.string(field.element, "AXValue"), draft)
    }
}
