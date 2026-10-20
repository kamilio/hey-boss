import AppKit
import XCTest
@testable import HeyTalk

final class OverlayTests: XCTestCase {
    @MainActor func testNoticeExpandsInsideRecordingPanelAndDismissesBeforeRecording() throws {
        let app = NSApplication.shared
        let oldWindows = Set(app.windows.map(\.windowNumber))
        let overlay = Overlay()
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(app.windows.first { !oldWindows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        let initialFrame = panel.frame
        let windowCount = app.windows.count
        func notices(_ view: NSView) -> [OverlayNotice] {
            (view as? OverlayNotice).map { [$0] } ?? view.subviews.flatMap(notices)
        }
        overlay.error("Copied. Press ⌘V to paste.")
        XCTAssertEqual(app.windows.count, windowCount)
        XCTAssertGreaterThan(panel.frame.height, initialFrame.height)
        XCTAssertEqual(panel.frame.minY, initialFrame.minY, accuracy: 1)
        XCTAssertEqual(panel.frame.midX, initialFrame.midX, accuracy: 1)
        XCTAssertEqual(notices(try XCTUnwrap(panel.contentView)).count, 1)
        overlay.error("Allow automatic paste. Text copied.", accessibilityAction: true)
        XCTAssertEqual(notices(try XCTUnwrap(panel.contentView)).count, 1)
        XCTAssertFalse(panel.canBecomeKey)
        overlay.reposition()
        let screen = NSScreen.screens.first { NSMouseInRect(NSEvent.mouseLocation, $0.frame, false) } ?? NSScreen.main
        XCTAssertEqual(panel.frame.midX, try XCTUnwrap(screen).visibleFrame.midX, accuracy: 1)
        let notice = try XCTUnwrap(notices(try XCTUnwrap(panel.contentView)).first)
        let close = try XCTUnwrap(notice.subviews.compactMap { $0 as? NSButton }.first { $0.accessibilityLabel() == "Dismiss notice" })
        close.performClick(nil)
        XCTAssertTrue(notices(try XCTUnwrap(panel.contentView)).isEmpty)
        XCTAssertEqual(panel.frame.size, initialFrame.size)
        overlay.error("Copied. Press ⌘V to paste.")
        overlay.update(.recording)
        XCTAssertTrue(notices(try XCTUnwrap(panel.contentView)).isEmpty)
        XCTAssertEqual(panel.frame.size, initialFrame.size)
    }

    @MainActor func testWaveformIsVisibleAsSoonAsRecordingIsReady() throws {
        let app = NSApplication.shared
        let oldWindows = Set(app.windows.map(\.windowNumber))
        let overlay = Overlay()
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(app.windows.first { !oldWindows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        func findWave(_ view: NSView) -> NSView? {
            if view.identifier?.rawValue == "recording-waveform" { return view }
            return view.subviews.lazy.compactMap(findWave).first
        }
        let wave = try XCTUnwrap(panel.contentView.flatMap(findWave))
        overlay.preparing()
        XCTAssertTrue(wave.isHidden)
        overlay.update(.recording)
        XCTAssertFalse(wave.isHidden)
        XCTAssertEqual(wave.alphaValue, 1)
        overlay.update(.idle)
        XCTAssertTrue(wave.isHidden)
    }
}
