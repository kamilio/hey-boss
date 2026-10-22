import AppKit
import TalkCore
import XCTest
@testable import HeyTalk

final class OverlayTests: XCTestCase {
    @MainActor func testBubbleMenuShowsCurrentModesAndShortcutsAndDispatchesTheRightAction() throws {
        let overlay = Overlay()
        defer { overlay.show(false) }
        var speech = true, auto = false, dictationStarts = 0
        overlay.speechEnabled = { speech }
        overlay.onToggleSpeech = { speech.toggle() }
        overlay.onToggleAuto = { auto.toggle(); overlay.setAutoMode(auto) }
        overlay.onToggle = { dictationStarts += 1 }
        let menu = overlay.contextMenu()
        let read = try XCTUnwrap(menu.items.first { $0.title == "Read Aloud · Hyper+T" })
        let automatic = try XCTUnwrap(menu.items.first { $0.title == "Auto Dictation · Hyper+V" })
        XCTAssertEqual(read.state, .on)
        XCTAssertEqual(automatic.state, .off)
        XCTAssertTrue(NSApplication.shared.sendAction(try XCTUnwrap(read.action), to: read.target, from: read))
        XCTAssertFalse(speech)
        XCTAssertTrue(NSApplication.shared.sendAction(try XCTUnwrap(automatic.action), to: automatic.target, from: automatic))
        XCTAssertTrue(auto)
        XCTAssertEqual(dictationStarts, 0, "Opening and using the menu must not start manual recording")
        let refreshed = overlay.contextMenu()
        XCTAssertEqual(refreshed.items.first { $0.title == read.title }?.state, .off)
        XCTAssertEqual(refreshed.items.first { $0.title == automatic.title }?.state, .on)
    }
    @MainActor func testCompactModeNoticeSurvivesCaptureStartingAndCloses() throws {
        let app = NSApplication.shared
        let oldWindows = Set(app.windows.map(\.windowNumber))
        let overlay = Overlay()
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(app.windows.first { !oldWindows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        func notices(_ view: NSView) -> [OverlayNotice] {
            (view as? OverlayNotice).map { [$0] } ?? view.subviews.flatMap(notices)
        }
        let root = try XCTUnwrap(panel.contentView)
        overlay.setAutoMode(true)
        overlay.showModeNotice()
        overlay.preparing(); overlay.update(.recording)
        let notice = try XCTUnwrap(notices(root).first)
        let message = try XCTUnwrap(notice.subviews.compactMap { $0 as? NSTextField }.first).stringValue
        XCTAssertEqual(message, "Auto mode on")
        XCTAssertEqual(notice.frame.width, 180)
        XCTAssertFalse(panel.canBecomeKey)
        let dismissed = expectation(description: "mode notice closes automatically")
        DispatchQueue.main.asyncAfter(deadline: .now() + 3.3) {
            XCTAssertTrue(notices(root).isEmpty)
            dismissed.fulfill()
        }
        wait(for: [dismissed], timeout: 4)
        overlay.setAutoMode(false); overlay.showModeNotice()
        let off = try XCTUnwrap(notices(root).first)
        XCTAssertEqual(try XCTUnwrap(off.subviews.compactMap { $0 as? NSTextField }.first).stringValue, "Auto mode off")
    }

    @MainActor func testAutoGlowStaysVisibleWithoutAddingTextOrChangingPillSize() throws {
        let app = NSApplication.shared
        let oldWindows = Set(app.windows.map(\.windowNumber))
        let overlay = Overlay()
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(app.windows.first { !oldWindows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        let root = try XCTUnwrap(panel.contentView)
        let glow = try XCTUnwrap(root.layer?.sublayers?.first { $0.name == "auto-mode-glow" } as? CAShapeLayer)
        XCTAssertTrue(glow.isHidden)
        for phase in [Phase.idle, .recording, .processing] {
            overlay.setAutoMode(false)
            overlay.update(phase)
            let manualSize = panel.frame.size
            overlay.setAutoMode(true)
            XCTAssertFalse(glow.isHidden)
            XCTAssertEqual(panel.frame.size, manualSize)
            XCTAssertGreaterThan(glow.shadowOpacity, 0)
            XCTAssertNotNil(glow.path)
        }
        overlay.setAutoMode(false)
        XCTAssertTrue(glow.isHidden)
        XCTAssertFalse(panel.canBecomeKey)
    }

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
        XCTAssertFalse(wave.isHidden, "Opening the microphone must show the recording bar immediately")
        XCTAssertLessThan(wave.alphaValue, 1, "Waiting for sound must look different from live audio")
        func descendants(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(descendants) }
        let views = descendants(try XCTUnwrap(panel.contentView))
        let waiting = try XCTUnwrap(views.compactMap { $0 as? NSTextField }.first { $0.stringValue == "Connecting…" })
        XCTAssertFalse(waiting.isHidden)
        XCTAssertFalse(views.contains { ($0 as? NSButton)?.toolTip == "Cancel · Esc" }, "No separate close button")
        overlay.update(.recording)
        XCTAssertFalse(wave.isHidden)
        XCTAssertEqual(wave.alphaValue, 1)
        XCTAssertTrue(waiting.isHidden)
        overlay.preparing()
        XCTAssertFalse(wave.isHidden, "Recovery must not collapse back to the idle icon")
        XCTAssertFalse(waiting.isHidden)
        overlay.update(.idle)
        XCTAssertTrue(wave.isHidden)
    }
}
