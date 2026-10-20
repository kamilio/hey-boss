import AppKit
import XCTest
@testable import HeyTalk

final class OverlayNoticeTests: XCTestCase {
    func testTechnicalErrorsBecomeShortRecoveryInstructions() {
        XCTAssertEqual(OverlayNotice.concise("No speech detected. Check your microphone with hey-talk devices."), "No speech detected. Try again.")
        XCTAssertEqual(OverlayNotice.concise("Microphone access is required. Run hey-talk permissions microphone."), "Allow microphone access in Settings.")
        XCTAssertEqual(OverlayNotice.concise("Selected microphone is disconnected."), "Microphone unavailable. Try reconnecting.")
        XCTAssertEqual(OverlayNotice.concise("No sound from the microphone. Check its mute control and the system input in Sound settings."), "No microphone sound. Check mute and Sound settings.")
        XCTAssertEqual(OverlayNotice.concise("Five-minute recording limit reached."), "Recording limit reached. Start again.")
        XCTAssertEqual(OverlayNotice.concise("Orukeet is missing. Run the hey-talk installer."), "Speech model missing. Reinstall Hey Talk.")
        XCTAssertEqual(OverlayNotice.concise("An internal runtime error with implementation details"), "Couldn’t transcribe. Try again.")
    }

    @MainActor func testPermissionActionOpensSettingsAndDismissDoesNot() {
        _ = NSApplication.shared
        var opened = 0, dismissed = 0
        let notice = OverlayNotice(message: "Accessibility is missing", openAccessibility: { opened += 1 }, dismiss: { dismissed += 1 })
        let buttons = notice.subviews.compactMap { $0 as? NSButton }
        let action = buttons.first { $0.title == "Enable Accessibility" }
        XCTAssertNotNil(action)
        XCTAssertGreaterThanOrEqual(action?.frame.width ?? 0, notice.bounds.width - 32)
        XCTAssertGreaterThanOrEqual(action?.frame.height ?? 0, 32)
        action?.performClick(nil)
        XCTAssertEqual(opened, 1)
        XCTAssertEqual(dismissed, 1)
        buttons.first { $0.accessibilityLabel() == "Dismiss notice" }?.performClick(nil)
        XCTAssertEqual(dismissed, 2)
        XCTAssertEqual(opened, 1)
    }

    @MainActor func testUnrelatedErrorsDoNotOfferPermissionChanges() {
        _ = NSApplication.shared
        var dismissed = false
        let notice = OverlayNotice(message: "The original app closed", openAccessibility: nil, dismiss: { dismissed = true })
        let buttons = notice.subviews.compactMap { $0 as? NSButton }
        XCTAssertFalse(buttons.contains { $0.title == "Enable Accessibility" })
        buttons.first { $0.accessibilityLabel() == "Dismiss notice" }?.performClick(nil)
        XCTAssertTrue(dismissed)
    }
}
