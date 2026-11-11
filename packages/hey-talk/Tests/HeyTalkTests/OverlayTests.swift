import AppKit
import TalkCore
import XCTest
@testable import HeyTalk

final class OverlayTests: XCTestCase {
    @MainActor func testChatOwnsPillUntilReleasedAndStartupIsImmediate() async throws {
        let app = NSApplication.shared, windows = Set(app.windows.map(\.windowNumber))
        let overlay = Overlay()
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(app.windows.first { !windows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        func views(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(views) }
        let all = views(try XCTUnwrap(panel.contentView))
        let surface = try XCTUnwrap(all.first { $0.layer?.sublayers?.contains { $0.name == "ai-disabled" } == true })
        overlay.setCorrection(false); overlay.setAutoSubmit(false)
        for (phase, message) in [("idle", "Voice off"), ("starting", "Starting…"), ("listening", "Listening"), ("recording", "Recording"), ("decoding", "Transcribing…"), ("thinking", "Thinking…"), ("speaking", "Speaking")] {
            overlay.showConversation(["phase": phase, "message": message, "action": "Stop voice"])
            XCTAssertTrue(surface.accessibilityLabel()?.contains(message) == true)
            XCTAssertFalse(surface.accessibilityLabel()?.contains("dictation") == true)
            XCTAssertFalse(all.compactMap { $0 as? NSTextField }.contains { !$0.isHidden && $0.stringValue == message })
            let icon = all.compactMap { $0 as? NSImageView }.first { !$0.isHidden && $0.image?.accessibilityDescription == message }
            let spinner = all.compactMap { $0 as? NSProgressIndicator }.first { !$0.isHidden }
            XCTAssertTrue(icon != nil || spinner != nil)
            XCTAssertTrue(surface.layer?.sublayers?.first { $0.name == "ai-disabled" }?.isHidden == true)
            XCTAssertFalse(panel.canBecomeKey)
        }
        var requests: [Bool] = []
        overlay.onPauseListening = { requests.append($0) }
        overlay.onToggle = { XCTFail("Pause must not send or stop the conversation") }
        for phase in ["listening", "recording"] {
            var position: NSRect?
            for paused in [false, true] {
                overlay.showConversation(["phase": phase, "message": paused ? "Listening paused" : "Listening", "listeningPaused": paused])
                let label = paused ? "Resume listening" : "Pause listening"
                let button = try XCTUnwrap(all.compactMap { $0 as? NSButton }.first { !$0.isHidden && $0.accessibilityLabel() == label })
                let rect = button.convert(button.bounds, to: panel.contentView)
                XCTAssertTrue(panel.contentView?.hitTest(NSPoint(x: rect.midX, y: rect.midY)) === button)
                if let position { XCTAssertEqual(position, rect) }
                position = rect
                button.performClick(nil)
            }
        }
        XCTAssertEqual(requests, [true, false, true, false])
        overlay.showConversation(["phase": "thinking", "message": "Needs approval", "action": "Open chat", "approval": true])
        XCTAssertEqual(overlay.contextMenu().items.map(\.title), ["Open chat", "Stop conversation", "New Chat"])
        overlay.showConversation(["phase": "speaking", "message": "Speaking", "action": "Stop voice", "interrupt": true])
        let interrupt = try XCTUnwrap(overlay.contextMenu().items.first { $0.title == "Listen for next message" })
        var interrupted = false; overlay.onInterruptConversation = { interrupted = true }
        XCTAssertTrue(app.sendAction(try XCTUnwrap(interrupt.action), to: interrupt.target, from: interrupt))
        XCTAssertTrue(interrupted)
        overlay.showConversation(["phase": "recording", "action": "Send voice message"])
        let menu = overlay.contextMenu()
        var stopped = false, reset = false
        overlay.onStopConversation = { stopped = true }; overlay.onStartConversation = { reset = true }
        for title in ["Stop recording", "New Chat"] {
            let item = try XCTUnwrap(menu.item(withTitle: title))
            XCTAssertTrue(app.sendAction(try XCTUnwrap(item.action), to: item.target, from: item))
        }
        XCTAssertTrue(stopped && reset)
        overlay.showConversation(nil)
        XCTAssertTrue(surface.accessibilityLabel()?.contains("Start dictation") == true)
        overlay.preparing(.listening)
        XCTAssertTrue(surface.accessibilityLabel()?.contains("Opening microphone") == true, "Startup state changes before the delayed Connecting label")
    }

    @MainActor func testT3ConfirmationIsSmallTemporaryAndLeavesDictationAlone() async throws {
        let app = NSApplication.shared, oldWindows = Set(app.windows.map(\.windowNumber))
        let overlay = Overlay()
        overlay.show(true)
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(app.windows.first { !oldWindows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        let root = try XCTUnwrap(panel.contentView), initial = panel.frame, windows = Set(app.windows.map(\.windowNumber))
        func views(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(views) }
        func label() -> NSTextField? {
            views(root).compactMap { $0 as? NSTextField }.first { !$0.isHidden && $0.stringValue == "Unpinned" }
        }
        func iconX() throws -> CGFloat {
            let icon = try XCTUnwrap(views(root).first { String(describing: type(of: $0)) == "IdentityView" })
            return panel.convertToScreen(icon.convert(icon.bounds, to: nil)).midX
        }
        try await Task.sleep(nanoseconds: 350_000_000)
        let anchor = try iconX()
        for action: CorrectionAction in [.paste("hello"), .clearInput, .listThreads] { overlay.confirmAction(action) }
        XCTAssertEqual(panel.frame, initial)
        overlay.confirmAction(.pinThread(true))
        overlay.confirmAction(.pinThread(false))
        overlay.finishProcessing()
        overlay.update(.listening)
        try await Task.sleep(nanoseconds: 400_000_000)
        let text = try XCTUnwrap(label())
        XCTAssertLessThan(panel.convertToScreen(text.convert(text.bounds, to: nil)).maxX, anchor - 11)
        XCTAssertEqual(try iconX(), anchor, accuracy: 1)
        XCTAssertTrue(app.windows.filter(\.isVisible).allSatisfy { windows.contains($0.windowNumber) })
        XCTAssertFalse(panel.canBecomeKey)
        XCTAssertEqual(panel.frame.height, initial.height)
        try await Task.sleep(nanoseconds: 3_200_000_000)
        XCTAssertNil(label())
        XCTAssertEqual(panel.frame, initial)
    }

    @MainActor func testWaitingPillNamesDestinationAndLetsUserCancelWithoutTakingFocus() throws {
        let app = NSApplication.shared, windows = Set(app.windows.map(\.windowNumber))
        let overlay = Overlay()
        defer { overlay.show(false) }
        let retry = Timer(timeInterval: 10, repeats: false) { _ in }
        let panel = try XCTUnwrap(app.windows.first { !windows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        func find(_ view: NSView) -> PasteWaitView? {
            (view as? PasteWaitView) ?? view.subviews.compactMap(find).first
        }
        let root = try XCTUnwrap(panel.contentView)
        let brief = Timer(timeInterval: 10, repeats: false) { _ in }
        overlay.waitToPaste("Transient focus loss", while: brief)
        brief.invalidate()
        func settle() {
            let settled = expectation(description: "Focus has settled")
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { settled.fulfill() }
            wait(for: [settled], timeout: 2)
        }
        settle()
        XCTAssertNil(find(root), "A recovered or cancelled paste must never show the waiting pill")
        overlay.waitToPaste("Focus Design review in T3", while: retry)
        XCTAssertNil(find(root), "The first missed focus check must be silent")
        settle()
        let waitView = try XCTUnwrap(find(root))
        XCTAssertFalse(panel.canBecomeKey)
        XCTAssertTrue(waitView.accessibilityLabel()?.contains("Focus Design review in T3") == true)
        let cancel = try XCTUnwrap(waitView.subviews.compactMap { $0 as? NSButton }.first)
        overlay.onCancelPaste = { overlay.update(.idle) }
        cancel.performClick(nil)
        XCTAssertNil(find(root))
    }

    @MainActor func testDashboardIndicatorsFollowModesWithoutTakingClicks() throws {
        let app = NSApplication.shared
        let oldWindows = Set(app.windows.map(\.windowNumber))
        let overlay = Overlay()
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(app.windows.first { !oldWindows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        func descendants(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(descendants) }
        let views = descendants(try XCTUnwrap(panel.contentView))
        let surface = try XCTUnwrap(views.first { $0.layer?.sublayers?.contains { $0.name == "ai-disabled" } == true })
        let ai = try XCTUnwrap(surface.layer?.sublayers?.first { $0.name == "ai-disabled" })
        let submit = try XCTUnwrap(surface.layer?.sublayers?.first { $0.name == "auto-submit" })
        XCTAssertTrue(ai.isHidden)
        XCTAssertTrue(submit.isHidden)
        XCTAssertNotNil(ai.contents)
        XCTAssertNotNil(submit.contents)
        for phase in [Phase.idle, .listening, .recording, .processing] {
            overlay.update(phase)
            if phase == .listening { XCTAssertTrue(surface.accessibilityLabel()?.contains("Listening") == true) }
            let size = panel.frame.size
            overlay.setCorrection(true)
            XCTAssertTrue(ai.isHidden)
            XCTAssertTrue(surface.accessibilityLabel()?.contains("AI correction on") == true)
            overlay.setCorrection(false)
            XCTAssertFalse(ai.isHidden)
            XCTAssertTrue(surface.accessibilityLabel()?.contains("AI correction off") == true)
            overlay.setAutoSubmit(false)
            XCTAssertFalse(submit.isHidden)
            XCTAssertTrue(surface.accessibilityLabel()?.contains("Auto Submit off") == true)
            XCTAssertFalse(ai.frame.intersects(submit.frame))
            XCTAssertTrue(surface.bounds.contains(ai.frame))
            XCTAssertTrue(surface.bounds.contains(submit.frame))
            XCTAssertGreaterThan(ai.bounds.height, 4)
            XCTAssertGreaterThan(submit.bounds.width, submit.bounds.height, "Return must keep its natural wide shape")
            XCTAssertEqual(ai.position.y, 26)
            XCTAssertEqual(submit.position.y, 10)
            XCTAssertEqual(panel.frame.size, size, "Indicators must not enlarge the pill")
            overlay.setAutoMode(true)
            XCTAssertFalse(submit.isHidden, "Auto Dictation must not override an app's Enter exception")
            overlay.setAutoMode(false)
            overlay.setAutoSubmit(true)
            XCTAssertTrue(submit.isHidden)
        }
        overlay.setAutoSubmit(false)
        overlay.update(.idle)
        XCTAssertFalse(submit.isHidden, "App exceptions remain visible while idle")
        var toggles = 0
        overlay.onToggleCorrection = { toggles += 1 }
        let item = try XCTUnwrap(overlay.contextMenu().items.first { $0.title == "AI Correction" })
        XCTAssertTrue(app.sendAction(try XCTUnwrap(item.action), to: item.target, from: item))
        XCTAssertEqual(toggles, 1)
    }

    @MainActor func testProcessingRimAdvancesAndStopsOnCancellation() throws {
        let app = NSApplication.shared
        let oldWindows = Set(app.windows.map(\.windowNumber))
        let overlay = Overlay()
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(app.windows.first { !oldWindows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        let root = try XCTUnwrap(panel.contentView)
        let rim = try XCTUnwrap(root.layer?.sublayers?.first { $0.name == "transcription-progress" } as? CAShapeLayer)
        overlay.processing(audioSeconds: 30, correction: true)
        XCTAssertFalse(rim.isHidden)
        let initial = rim.strokeEnd
        let tick = expectation(description: "progress advances")
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { tick.fulfill() }
        wait(for: [tick], timeout: 1)
        XCTAssertGreaterThan(rim.strokeEnd, initial)
        XCTAssertLessThan(rim.strokeEnd, 1)
        let beforeEstimate = rim.strokeEnd
        overlay.processing(audioSeconds: 240, correction: true)
        XCTAssertGreaterThanOrEqual(rim.strokeEnd, beforeEstimate, "Receiving the audio duration must not restart the progress animation")
        let updated = expectation(description: "updated estimate has ticked")
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { updated.fulfill() }
        wait(for: [updated], timeout: 1)
        XCTAssertGreaterThanOrEqual(rim.strokeEnd, beforeEstimate)
        overlay.update(.idle)
        XCTAssertTrue(rim.isHidden)
        XCTAssertNil(rim.animationKeys())
    }

    @MainActor func testBubbleMenuShowsCurrentModesAndShortcutsAndDispatchesTheRightAction() throws {
        let overlay = Overlay()
        defer { overlay.show(false) }
        var speech = true, auto = false, submit = true, dictationStarts = 0
        overlay.speechEnabled = { speech }
        overlay.onToggleSpeech = { speech.toggle() }
        overlay.onToggleAuto = { auto.toggle(); overlay.setAutoMode(auto) }
        overlay.onToggleAutoSubmit = { submit.toggle(); overlay.setAutoSubmit(submit) }
        overlay.onToggle = { dictationStarts += 1 }
        overlay.update(.recording)
        var historyOpened = false
        overlay.onShowHistory = { historyOpened = true }
        let menu = overlay.contextMenu()
        XCTAssertEqual(menu.items.map(\.title), ["Dictation History…", "Vocabulary…", "", "Dictation", "AI Correction", "Auto Submit", "Auto Dictation", "Return to Field Before Pasting", "", "Speech", "Read Aloud", "", "Conversation", "Start Conversation", "Chat with Context", "", "Setup", "Paste Permissions…"])
        let returnItem = try XCTUnwrap(menu.item(withTitle: "Return to Field Before Pasting"))
        var shouldReturn = false
        overlay.onToggleReturnToField = { shouldReturn.toggle() }
        overlay.returnToFieldEnabled = { shouldReturn }
        XCTAssertEqual(returnItem.state, .off)
        XCTAssertTrue(NSApplication.shared.sendAction(try XCTUnwrap(returnItem.action), to: returnItem.target, from: returnItem))
        XCTAssertEqual(overlay.contextMenu().item(withTitle: returnItem.title)?.state, .on)
        let history = try XCTUnwrap(menu.items.first)
        XCTAssertTrue(NSApplication.shared.sendAction(try XCTUnwrap(history.action), to: history.target, from: history))
        XCTAssertTrue(historyOpened)
        XCTAssertEqual(dictationStarts, 0, "Opening History must not toggle the recording")
        let read = try XCTUnwrap(menu.items.first { $0.title == "Read Aloud" })
        let automatic = try XCTUnwrap(menu.items.first { $0.title == "Auto Dictation" })
        let submission = try XCTUnwrap(menu.items.first { $0.title == "Auto Submit" })
        for (item, key) in [(read, "t"), (automatic, "v"), (submission, "e")] {
            XCTAssertEqual(item.keyEquivalent, key)
            XCTAssertEqual(item.keyEquivalentModifierMask, [.control, .command, .option, .shift])
        }
        XCTAssertEqual(read.state, .on)
        XCTAssertEqual(automatic.state, .off)
        XCTAssertTrue(NSApplication.shared.sendAction(try XCTUnwrap(read.action), to: read.target, from: read))
        XCTAssertFalse(speech)
        XCTAssertTrue(NSApplication.shared.sendAction(try XCTUnwrap(automatic.action), to: automatic.target, from: automatic))
        XCTAssertTrue(auto)
        XCTAssertEqual(submission.state, .on)
        XCTAssertTrue(NSApplication.shared.sendAction(try XCTUnwrap(submission.action), to: submission.target, from: submission))
        XCTAssertFalse(submit)
        XCTAssertEqual(dictationStarts, 0, "Opening and using the menu must not start manual recording")
        let refreshed = overlay.contextMenu()
        XCTAssertEqual(refreshed.items.first { $0.title == read.title }?.state, .off)
        XCTAssertEqual(refreshed.items.first { $0.title == automatic.title }?.state, .on)
        XCTAssertEqual(refreshed.items.first { $0.title == submission.title }?.state, .off)
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
        overlay.showStatus("Auto mode on")
        overlay.preparing(); overlay.update(.recording)
        let notice = try XCTUnwrap(notices(root).first)
        let message = try XCTUnwrap(notice.subviews.compactMap { $0 as? NSTextField }.first).stringValue
        XCTAssertEqual(message, "Auto mode on")
        XCTAssertEqual(notice.frame.height, 36)
        XCTAssertLessThan(notice.frame.width, 140)
        XCTAssertGreaterThanOrEqual(notice.frame.minX, TalkDesign.recordingSize.width - 8)
        XCTAssertFalse(panel.canBecomeKey)
        let dismissed = expectation(description: "mode notice closes automatically")
        DispatchQueue.main.asyncAfter(deadline: .now() + 3.3) {
            XCTAssertTrue(notices(root).isEmpty)
            dismissed.fulfill()
        }
        wait(for: [dismissed], timeout: 4)
        overlay.setAutoMode(false); overlay.showStatus("Auto mode off")
        let off = try XCTUnwrap(notices(root).first)
        XCTAssertEqual(try XCTUnwrap(off.subviews.compactMap { $0 as? NSTextField }.first).stringValue, "Auto mode off")
        overlay.update(.listening)
        overlay.confirmAction(.pinThread(true))
        XCTAssertNotNil(off.superview)
        let activity = try XCTUnwrap(off.superview?.subviews.compactMap { $0 as? NSTextField }.first { $0.stringValue == "Pinned" })
        XCTAssertEqual(activity.textColor, .labelColor)
        let symbol = try XCTUnwrap(off.superview?.subviews.compactMap { $0 as? NSImageView }.first { !$0.isHidden })
        let capCenter = activity.frame.maxY - activity.firstBaselineOffsetFromTop + (activity.font?.capHeight ?? 0) / 2
        XCTAssertEqual(symbol.frame.midY, capCenter, accuracy: 0.5)
        XCTAssertEqual(symbol.contentTintColor, TalkDesign.success)
        let face = try XCTUnwrap(off.superview as? GlassFace)
        XCTAssertEqual(face.accent, TalkDesign.success)
        let close = try XCTUnwrap(off.subviews.compactMap { $0 as? NSButton }.first)
        close.performClick(nil)
        XCTAssertTrue(notices(root).isEmpty)
        XCTAssertFalse(activity.isHidden)
        overlay.confirmAction(.pinThread(false), failed: true)
        XCTAssertEqual(activity.stringValue, "Couldn’t unpin")
        XCTAssertEqual(symbol.contentTintColor, TalkDesign.failure)
        XCTAssertEqual(face.accent, TalkDesign.failure)
        overlay.update(.recording)
        XCTAssertTrue(symbol.isHidden)
        XCTAssertNotEqual(face.accent, TalkDesign.failure)
        XCTAssertEqual(panel.frame.height, TalkDesign.idleSize.height + 48)
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
        let plain = try XCTUnwrap(notices(try XCTUnwrap(panel.contentView)).first)
        let shortHeight = plain.frame.height
        var retried = false
        overlay.error("AI correction timed out.", retry: { retried = true })
        let retryNotice = try XCTUnwrap(notices(try XCTUnwrap(panel.contentView)).first)
        let retry = try XCTUnwrap(retryNotice.subviews.compactMap { $0 as? NSButton }.first { $0.accessibilityLabel() == "Retry" })
        let closeRetry = try XCTUnwrap(retryNotice.subviews.compactMap { $0 as? NSButton }.first { $0.accessibilityLabel() == "Dismiss notice" })
        XCTAssertEqual(retry.frame.size, closeRetry.frame.size)
        XCTAssertEqual(retry.frame.midY, closeRetry.frame.midY)
        XCTAssertLessThanOrEqual(retry.frame.maxX, closeRetry.frame.minX)
        XCTAssertLessThanOrEqual(retryNotice.frame.height, shortHeight)
        retry.performClick(nil)
        XCTAssertTrue(retried)
        XCTAssertNil(retryNotice.superview)
        overlay.error("The microphone is unavailable. Reconnect it and try recording again.")
        XCTAssertGreaterThan(try XCTUnwrap(notices(try XCTUnwrap(panel.contentView)).first).frame.height, shortHeight)
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

    @MainActor func testAutoListeningStartupDoesNotFlashRecordingWhenSwitchingChats() async throws {
        let app = NSApplication.shared
        let oldWindows = Set(app.windows.map(\.windowNumber))
        let overlay = Overlay()
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(app.windows.first { !oldWindows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        func descendants(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(descendants) }
        let views = descendants(try XCTUnwrap(panel.contentView))
        let wave = try XCTUnwrap(views.first { $0.identifier?.rawValue == "recording-waveform" })
        let connecting = try XCTUnwrap(views.compactMap { $0 as? NSTextField }.first { $0.stringValue == "Connecting…" })
        let glow = try XCTUnwrap(panel.contentView?.layer?.sublayers?.first { $0.name == "auto-mode-glow" } as? CAShapeLayer)
        overlay.setAutoMode(true)
        overlay.update(.listening)
        let compact = panel.frame.size
        let outline = glow.path?.boundingBox
        for _ in 0..<3 {
            overlay.update(.idle)
            overlay.preparing(.listening)
            XCTAssertTrue(wave.isHidden)
            XCTAssertTrue(connecting.isHidden)
            XCTAssertEqual(panel.frame.size, compact)
            XCTAssertEqual(glow.path?.boundingBox, outline)
            overlay.update(.listening)
            XCTAssertEqual(panel.frame.size, compact)
        }
        try await Task.sleep(for: .milliseconds(800))
        XCTAssertTrue(connecting.isHidden, "A finished startup must not flash a delayed Connecting notice")
        XCTAssertEqual(panel.frame.size, compact)
        overlay.update(.recording)
        XCTAssertFalse(wave.isHidden)
        XCTAssertGreaterThan(try XCTUnwrap(glow.path).boundingBox.width, try XCTUnwrap(outline).width)
    }

    @MainActor func testHoverRevealsAndDismissesConversationAction() async throws {
        try XCTSkipIf(NSWorkspace.shared.accessibilityDisplayShouldReduceMotion, "Requires motion")
        let windows = Set(NSApplication.shared.windows.map(\.windowNumber))
        let overlay = Overlay()
        overlay.show(true)
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(NSApplication.shared.windows.first { !windows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        func descendants(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(descendants) }
        let button = try XCTUnwrap(descendants(try XCTUnwrap(panel.contentView)).compactMap { $0 as? NSButton }.first { $0.accessibilityLabel() == "Start conversation" })
        let face = try XCTUnwrap(button.superview)
        func checkTransition() async throws {
            for _ in 0..<20 {
                try await Task.sleep(for: .milliseconds(10))
                if !button.isHidden, let opacity = button.layer?.presentation()?.opacity, opacity > 0, opacity < 1,
                   let width = face.layer?.presentation()?.bounds.width, width > TalkDesign.idleSize.width, width < 84 { return }
            }
            XCTFail("The conversation icon should fade as the capsule changes width")
        }
        face.mouseEntered(with: NSEvent())
        try await checkTransition()
        try await Task.sleep(for: .milliseconds(320))
        XCTAssertTrue(button.isEnabled)
        face.mouseExited(with: NSEvent())
        XCTAssertFalse(button.isEnabled)
        try await checkTransition()
        face.mouseEntered(with: NSEvent())
        try await Task.sleep(for: .milliseconds(320))
        XCTAssertFalse(button.isHidden, "An interrupted dismissal must leave the action visible")
        face.mouseExited(with: NSEvent())
        try await Task.sleep(for: .milliseconds(320))
        XCTAssertTrue(button.isHidden)
    }

    @MainActor func testRightStatusOpensAndClosesWithoutMovingThePill() async throws {
        guard !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion else { throw XCTSkip("Requires motion") }
        let windows = Set(NSApplication.shared.windows.map(\.windowNumber))
        let overlay = Overlay()
        overlay.show(true)
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(NSApplication.shared.windows.first { !windows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        let compact = panel.frame
        overlay.setCorrection(false)
        overlay.showStatus("AI correction off")
        func status(in view: NSView) -> OverlayNotice? {
            view as? OverlayNotice ?? view.subviews.lazy.compactMap(status(in:)).first
        }
        let root = try XCTUnwrap(panel.contentView)
        let first = try XCTUnwrap(status(in: root))
        XCTAssertEqual(panel.frame.minX, compact.minX, accuracy: 1, "Opening to the right should keep the window origin stable")
        let face = try XCTUnwrap(first.superview?.layer)
        func checkTransition() async throws {
            for _ in 0..<15 {
                try await Task.sleep(for: .milliseconds(10))
                if let opacity = first.layer?.presentation()?.opacity, opacity > 0, opacity < 1,
                   let width = face.presentation()?.bounds.width, width > TalkDesign.idleSize.width + 1,
                   width < TalkDesign.idleSize.width + first.frame.width - 1 { return }
            }
            XCTFail("The text and pill should pass through intermediate frames together")
        }
        try await checkTransition()
        try await Task.sleep(for: .milliseconds(350))
        let close = try XCTUnwrap(first.subviews.compactMap { $0 as? NSButton }.first)
        close.performClick(nil)
        XCTAssertNotNil(first.superview, "Keep the message visible while the pill closes")
        try await checkTransition()
        overlay.showStatus("AI correction on")
        try await Task.sleep(for: .milliseconds(400))
        XCTAssertEqual(status(in: root)?.accessibilityLabel(), "AI correction on")
        let next = try XCTUnwrap(status(in: root))
        try XCTUnwrap(next.subviews.compactMap { $0 as? NSButton }.first).performClick(nil)
        try await Task.sleep(for: .milliseconds(400))
        XCTAssertNil(status(in: root))
        XCTAssertEqual(panel.frame.minX, compact.minX, accuracy: 1)
        XCTAssertEqual(panel.frame.size, compact.size)
    }

    @MainActor func testVisibleDrawerHasRoomToAnimateClosed() async throws {
        guard !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion else { throw XCTSkip("Requires motion") }
        let windows = Set(NSApplication.shared.windows.map(\.windowNumber))
        let overlay = Overlay()
        overlay.show(true)
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(NSApplication.shared.windows.first { !windows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        let compact = panel.frame.size
        overlay.error("Reconnect your microphone and try again.")
        try await Task.sleep(for: .milliseconds(350))
        let open = panel.frame.size
        XCTAssertGreaterThan(open.height, compact.height)
        overlay.update(.listening)
        XCTAssertEqual(panel.frame.size, open, "Shrinking the window immediately clips the closing glass")
        try await Task.sleep(for: .milliseconds(420))
        XCTAssertEqual(panel.frame.size, compact)
    }

    @MainActor func testVisibleWaveformMovesBetweenMicrophoneSamples() async throws {
        guard !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion else { throw XCTSkip("Requires motion") }
        let windows = Set(NSApplication.shared.windows.map(\.windowNumber))
        let overlay = Overlay()
        overlay.show(true)
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(NSApplication.shared.windows.first { !windows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        func waveform(_ view: NSView) -> NSView? {
            view.identifier?.rawValue == "recording-waveform" ? view : view.subviews.lazy.compactMap(waveform).first
        }
        let bar = try XCTUnwrap(panel.contentView.flatMap(waveform)?.layer?.sublayers?.last)
        var samples = 0
        overlay.level = { samples += 1; return samples.isMultiple(of: 2) ? 0 : 1 }
        overlay.update(.recording)
        var intermediate = Set<Int>()
        for _ in 0..<50 {
            try await Task.sleep(for: .milliseconds(10))
            if let h = bar.presentation()?.bounds.height, h > 4.2, h < 21.8 { intermediate.insert(Int(h * 10)) }
        }
        XCTAssertGreaterThan(intermediate.count, 3, "Bars should pass through intermediate heights between the quiet and loud samples")
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
        overlay.level = { 0.0058 }
        overlay.update(.recording)
        XCTAssertFalse(wave.isHidden)
        XCTAssertEqual(wave.alphaValue, 1)
        XCTAssertTrue(waiting.isHidden)
        XCTAssertEqual(panel.frame.size, recordingSize)
        // Check the drawn bars after real meter ticks, not just the view's visibility.
        let live = expectation(description: "Quiet microphone input reaches the waveform")
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
            if let pixels = wave.bitmapImageRepForCachingDisplay(in: wave.bounds) {
                wave.cacheDisplay(in: wave.bounds, to: pixels)
                let x = pixels.pixelsWide * 51 / 52
                let rows = (0..<pixels.pixelsHigh).filter { (pixels.colorAt(x: x, y: $0)?.alphaComponent ?? 0) > 0.5 }
                XCTAssertEqual(Double(rows.count) * wave.bounds.height / Double(pixels.pixelsHigh), 11, accuracy: 1)
            } else { XCTFail("Could not render recording bars") }
            live.fulfill()
        }
        wait(for: [live], timeout: 2)
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
