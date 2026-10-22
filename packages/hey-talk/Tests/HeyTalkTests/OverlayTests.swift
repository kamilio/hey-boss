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
        XCTAssertTrue(waiting.isHidden, "Brief microphone startup must not flash the wide Connecting bubble")
        let recordingSize = panel.frame.size
        XCTAssertFalse(views.contains { ($0 as? NSButton)?.toolTip == "Cancel · Esc" }, "No separate close button")
        overlay.update(.recording)
        XCTAssertFalse(wave.isHidden)
        XCTAssertEqual(wave.alphaValue, 1)
        XCTAssertTrue(waiting.isHidden)
        XCTAssertEqual(panel.frame.size, recordingSize)
        overlay.preparing()
        XCTAssertFalse(wave.isHidden, "Recovery must not collapse back to the idle icon")
        let early = expectation(description: "Connecting stays hidden before 700 ms")
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
            XCTAssertTrue(waiting.isHidden)
            XCTAssertEqual(panel.frame.size, recordingSize)
            early.fulfill()
        }
        let slow = expectation(description: "Slow microphone recovery shows Connecting")
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
            XCTAssertFalse(waiting.isHidden)
            XCTAssertGreaterThan(panel.frame.width, recordingSize.width)
            slow.fulfill()
        }
        wait(for: [early, slow], timeout: 2)
        overlay.update(.idle)
        XCTAssertTrue(wave.isHidden)
        overlay.preparing()
        overlay.update(.idle)
        let cancelled = expectation(description: "Cancelled startup cannot reopen Connecting")
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
            XCTAssertTrue(waiting.isHidden)
            XCTAssertTrue(wave.isHidden)
            cancelled.fulfill()
        }
        wait(for: [cancelled], timeout: 2)
    }

    @MainActor func testDockLiftsBubbleWithoutDriftingAndRestoresItsPosition() throws {
        let app = NSApplication.shared
        let oldWindows = Set(app.windows.map(\.windowNumber))
        let overlay = Overlay()
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(app.windows.first { !oldWindows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        let screen = try XCTUnwrap(panel.screen).frame
        panel.setFrameOrigin(NSPoint(x: screen.midX - 180, y: screen.minY + 12))
        let original = panel.frame
        let dock = NSRect(x: screen.midX - 300, y: screen.minY, width: 600, height: 80)
        overlay.avoidDock(dock)
        XCTAssertEqual(panel.frame.minX, original.minX)
        XCTAssertEqual(panel.frame.minY, dock.maxY + 12)
        overlay.avoidDock(dock)
        XCTAssertEqual(panel.frame.minY, dock.maxY + 12, "Repeated checks must not push it farther up")
        let floatingDock = dock.offsetBy(dx: 0, dy: 10)
        overlay.avoidDock(floatingDock)
        XCTAssertEqual(panel.frame.minY, floatingDock.maxY + 12, "The fully opened Dock floats above the screen edge")
        overlay.avoidDock(floatingDock)
        XCTAssertEqual(panel.frame.minY, floatingDock.maxY + 12, "The bubble must stay raised while the floating Dock stays open")
        overlay.update(.recording)
        overlay.avoidDock(dock)
        XCTAssertEqual(panel.frame.minY, dock.maxY + 12)
        overlay.avoidDock(nil)
        XCTAssertEqual(panel.frame.origin, original.origin)
        overlay.avoidDock(dock.offsetBy(dx: 0, dy: -dock.height))
        XCTAssertEqual(panel.frame.origin, original.origin, "An off-screen Dock is hidden")
        overlay.avoidDock(dock.offsetBy(dx: screen.width, dy: 0))
        XCTAssertEqual(panel.frame.origin, original.origin, "A Dock on another screen must not move this bubble")
        panel.setFrameOrigin(NSPoint(x: original.minX, y: screen.minY + 200))
        overlay.avoidDock(dock)
        XCTAssertEqual(panel.frame.minY, screen.minY + 200, "A bubble already clear of the Dock stays put")
    }

    @MainActor func testDockMotionInterpolatesRetargetsAndPreservesResizing() throws {
        try XCTSkipIf(NSWorkspace.shared.accessibilityDisplayShouldReduceMotion, "Motion is disabled in system settings")
        let app = NSApplication.shared
        let oldWindows = Set(app.windows.map(\.windowNumber))
        let overlay = Overlay()
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(app.windows.first { !oldWindows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        let screen = try XCTUnwrap(panel.screen).frame
        panel.setFrameOrigin(NSPoint(x: screen.midX - 100, y: screen.minY + 12))
        panel.orderFrontRegardless()
        let original = panel.frame
        let dock = NSRect(x: screen.midX - 300, y: screen.minY + 10, width: 600, height: 72)
        func advance(_ seconds: TimeInterval) {
            let tick = expectation(description: "animation advances")
            DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { tick.fulfill() }
            wait(for: [tick], timeout: seconds + 1)
        }
        overlay.avoidDock(dock)
        XCTAssertEqual(panel.frame.minY, original.minY, accuracy: 1, "Motion must begin at the current position, without a jump")
        advance(0.06)
        XCTAssertGreaterThan(panel.frame.minY, original.minY)
        XCTAssertLessThan(panel.frame.minY, dock.maxY + 12, "The window must pass through intermediate positions")
        let during = panel.frame.minY
        overlay.avoidDock(dock)
        XCTAssertEqual(panel.frame.minY, during, accuracy: 1, "Polling must not jump or restart the motion")
        overlay.error("Test notice")
        let expandedSize = panel.frame.size
        advance(0.2)
        XCTAssertEqual(panel.frame.minY, dock.maxY + 12, accuracy: 1)
        XCTAssertEqual(panel.frame.size, expandedSize, "Motion must preserve a notice that resizes the bubble")
        XCTAssertEqual(panel.frame.midX, original.midX, accuracy: 1)
        overlay.avoidDock(nil)
        advance(0.06)
        XCTAssertLessThan(panel.frame.minY, dock.maxY + 12)
        XCTAssertGreaterThan(panel.frame.minY, original.minY)
        let returning = panel.frame.minY
        overlay.avoidDock(dock)
        XCTAssertEqual(panel.frame.minY, returning, accuracy: 1, "Reopening the Dock must continue from the current position")
        advance(0.25)
        XCTAssertEqual(panel.frame.minY, dock.maxY + 12, accuracy: 1)
        overlay.avoidDock(nil)
        advance(0.25)
        XCTAssertEqual(panel.frame.minY, original.minY, accuracy: 1, "Interruptions must not change the resting height")
    }
}
