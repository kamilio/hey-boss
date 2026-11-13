import AppKit
import XCTest
import Vision
@testable import HeyTalk

final class OverlayNoticeTests: XCTestCase {
    @MainActor func testWrappedNoticeRendersTheLastWord() throws {
        _ = NSApplication.shared
        let message = "This T3 thread is on another environment."
        let notice = OverlayNotice(message: message, dismiss: {})
        let label = try XCTUnwrap(notice.subviews.compactMap { $0 as? NSTextField }.first)
        let rendered = NSImage(size: notice.bounds.size, flipped: false) { bounds in
            NSColor.white.setFill(); bounds.fill()
            label.textColor = .black
            notice.displayIgnoringOpacity(notice.bounds, in: NSGraphicsContext.current!)
            return true
        }
        let image = try XCTUnwrap(rendered.cgImage(forProposedRect: nil, context: nil, hints: nil))
        let words = VNRecognizeTextRequest()
        try VNImageRequestHandler(cgImage: image).perform([words])
        let text = (words.results ?? []).compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        XCTAssertTrue(text.contains("environment"), text)
    }

    @MainActor func testNoticeMeasuresWrappedTextInsideItsActionColumn() throws {
        _ = NSApplication.shared
        for message in ["This T3 thread is on another environment.", Array(repeating: "A longer notice needs room to wrap.", count: 6).joined(separator: " ")] {
            let notice = OverlayNotice(message: message, retry: {}, dismiss: {})
            let label = try XCTUnwrap(notice.subviews.compactMap { $0 as? NSTextField }.first)
            let measured = NSTextField(wrappingLabelWithString: message)
            measured.font = label.font
            let needed = measured.cell!.cellSize(forBounds: NSRect(x: 0, y: 0, width: label.bounds.width, height: 10000))
            XCTAssertGreaterThanOrEqual(label.bounds.height, needed.height)
            XCTAssertTrue(notice.bounds.contains(label.frame))
        }
    }

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
        let notice = OverlayNotice(message: "Accessibility is missing", action: ("Enable Accessibility", { opened += 1 }), dismiss: { dismissed += 1 })
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
        let notice = OverlayNotice(message: "The original app closed", dismiss: { dismissed = true })
        let buttons = notice.subviews.compactMap { $0 as? NSButton }
        XCTAssertFalse(buttons.contains { $0.title == "Enable Accessibility" })
        buttons.first { $0.accessibilityLabel() == "Dismiss notice" }?.performClick(nil)
        XCTAssertTrue(dismissed)
    }
}
