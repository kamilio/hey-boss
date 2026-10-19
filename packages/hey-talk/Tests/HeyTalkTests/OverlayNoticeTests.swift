import AppKit
import XCTest
@testable import HeyTalk

final class OverlayNoticeTests: XCTestCase {
    @MainActor func testPermissionActionOpensSettingsAndDismissDoesNot() {
        _ = NSApplication.shared
        var opened = 0, dismissed = 0
        let notice = OverlayNotice(message: "Accessibility is missing", openAccessibility: { opened += 1 }, dismiss: { dismissed += 1 })
        let buttons = notice.subviews.compactMap { $0 as? NSButton }
        buttons.first { $0.title == "Open Accessibility Settings" }?.performClick(nil)
        XCTAssertEqual(opened, 1)
        XCTAssertEqual(dismissed, 1)
        buttons.first { $0.title == "Dismiss" }?.performClick(nil)
        XCTAssertEqual(dismissed, 2)
        XCTAssertEqual(opened, 1)
    }

    @MainActor func testUnrelatedErrorsDoNotOfferPermissionChanges() {
        _ = NSApplication.shared
        let notice = OverlayNotice(message: "The original app closed", openAccessibility: nil, dismiss: {})
        XCTAssertFalse(notice.subviews.compactMap { $0 as? NSButton }.contains { $0.title == "Open Accessibility Settings" })
    }
}
