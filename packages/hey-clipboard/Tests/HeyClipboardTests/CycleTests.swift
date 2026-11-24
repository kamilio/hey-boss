import AppKit
import XCTest
@testable import HeyClipboard

final class CycleTests: XCTestCase {
    private var directory: URL!
    private var history: History!
    private var model: ClipboardApp!
    private var previousApp: NSRunningApplication?

    override func setUpWithError() throws {
        _ = NSApplication.shared
        directory = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
            .appendingPathComponent(".build/cycle-test-" + UUID().uuidString)
        history = try History(directory: directory)
        model = ClipboardApp(history: history)
    }
    override func tearDownWithError() throws {
        model.dismiss()
        for window in NSApp.windows where window is PickerPanel { window.close() }
        model = nil
        history.finishWrites()
        try FileManager.default.removeItem(at: directory)
        previousApp?.activate()
    }
    func testRepeatedVAdvancesAndWrapsWithoutChangingHistory() {
        for text in ["oldest", "middle", "newest"] { history.add(text, app: "Test") }
        let original = history.clips
        model.cycle()
        XCTAssertTrue(model.cycling)
        XCTAssertEqual(model.selection?.text, "newest")
        model.flagsChanged([.command, .shift])
        XCTAssertTrue(model.cycling)
        model.cycle()
        XCTAssertEqual(model.selection?.text, "middle")
        model.cycle()
        XCTAssertEqual(model.selection?.text, "oldest")
        model.cycle()
        XCTAssertEqual(model.selection?.text, "newest")
        XCTAssertEqual(history.clips, original)
    }
    func testReusingOlderClipMakesItFirstOnNextOpenAndAfterRestart() throws {
        for text in ["oldest", "middle", "newest"] { history.add(text, app: "Source") }
        model.cycle()
        model.move(1)
        let chosen = try XCTUnwrap(model.selection)
        XCTAssertEqual(chosen.text, "middle")
        history.pin(chosen.id, true)
        // Both UI and CLI pastes use this copy path. Keep test text off the user's clipboard.
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        let monitor = ClipboardMonitor(history: history, board: board)
        XCTAssertTrue(monitor.copy(chosen))
        monitor.poll(app: "Destination")
        model.dismiss()
        model.cycle()
        XCTAssertEqual(model.selection?.id, chosen.id)
        XCTAssertEqual(history.clips.map(\.text), ["middle", "newest", "oldest"])
        XCTAssertEqual(history.clips.first?.app, "Source")
        XCTAssertEqual(history.clips.first?.pinned, true)
        history.finishWrites()
        XCTAssertEqual(try History(directory: directory).clips, history.clips)
    }
    func testReleasingBothModifiersEndsCycleOnlyOnce() {
        // Empty history exercises release without touching the real clipboard.
        for remaining: NSEvent.ModifierFlags in [.command, .shift] {
            model.cycle()
            XCTAssertTrue(model.cycling)
            model.flagsChanged(remaining)
            XCTAssertTrue(model.cycling)
            model.flagsChanged([])
            XCTAssertFalse(model.cycling)
            model.message = "already handled"
            model.flagsChanged([])
            XCTAssertEqual(model.message, "already handled")
            model.dismiss()
        }
    }
    func testVKeyMovesDownWithEitherOrBothModifiersHeld() throws {
        for text in ["oldest", "middle", "newest"] { history.add(text, app: "Test") }
        model.cycle()
        let panel = try XCTUnwrap(NSApp.windows.first { $0 is PickerPanel && $0.isVisible })
        for (flags, expected): (NSEvent.ModifierFlags, String) in [([.command, .shift], "middle"), (.command, "oldest"), (.shift, "newest")] {
            model.flagsChanged(flags)
            panel.sendEvent(try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero,
                modifierFlags: flags, timestamp: 0, windowNumber: panel.windowNumber,
                context: nil, characters: "v", charactersIgnoringModifiers: "v", isARepeat: false, keyCode: 9)))
            XCTAssertEqual(model.selection?.text, expected)
            XCTAssertTrue(model.cycling)
        }
    }
    func testImagesParticipateInTheSameCycle() throws {
        history.add("older text", app: "Test")
        let image = try XCTUnwrap(history.add(Data([1, 2, 3]), type: .png, app: "Image source"))
        history.add("newest text", app: "Test")
        model.cycle()
        XCTAssertEqual(model.selection?.text, "newest text")
        model.cycle()
        XCTAssertEqual(model.selection?.id, image.id)
        model.cycle()
        XCTAssertEqual(model.selection?.text, "older text")
    }
    func testEscapeCancelsWhileModifiersRemainHeld() throws {
        model.cycle()
        let panel = try XCTUnwrap(NSApp.windows.first { $0 is PickerPanel && $0.isVisible })
        let escape = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero,
            modifierFlags: [.command, .shift], timestamp: 0, windowNumber: panel.windowNumber,
            context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}",
            isARepeat: false, keyCode: 53))
        panel.sendEvent(escape)
        XCTAssertFalse(model.cycling)
        XCTAssertFalse(panel.isVisible)
        model.flagsChanged([])
        XCTAssertNil(model.message)
    }
    func testArrowKeysAndEnterWorkWithShortcutModifiersHeld() throws {
        history.add("older", app: "Test")
        history.add("newer", app: "Test")
        model.openPicker()
        let panel = try XCTUnwrap(NSApp.windows.first { $0 is PickerPanel && $0.isVisible })
        XCTAssertTrue(panel.isVisible)
        XCTAssertFalse(model.cycling)
        func key(_ code: UInt16, flags: NSEvent.ModifierFlags = []) throws {
            panel.sendEvent(try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero,
                modifierFlags: flags, timestamp: 0, windowNumber: panel.windowNumber,
                context: nil, characters: "", charactersIgnoringModifiers: "",
                isARepeat: false, keyCode: code)))
        }
        try key(125)
        XCTAssertEqual(model.selection?.text, "older")
        try key(126, flags: [.command, .shift])
        XCTAssertEqual(model.selection?.text, "newer")
        // Enter reaches the paste handler, without writing any test clip globally.
        history.clear()
        try key(36, flags: [.command, .shift])
        XCTAssertEqual(model.message, "Choose a destination app, then open the picker again.")
    }
    func testQuickPickerAndSearchHistoryHaveSeparateModesAndSizes() throws {
        // Empty history keeps mode transitions independent of the real clipboard.
        model.cycle()
        let panel = try XCTUnwrap(NSApp.windows.first { $0 is PickerPanel && $0.isVisible })
        XCTAssertFalse(model.browsing)
        XCTAssertTrue(panel.styleMask.contains(.nonactivatingPanel))
        XCTAssertEqual(panel.level, .popUpMenu)
        XCTAssertFalse(panel.hidesOnDeactivate)
        XCTAssertEqual(panel.frame.size, NSSize(width: 480, height: 300))
        model.openPicker()
        XCTAssertTrue(model.browsing)
        XCTAssertFalse(model.cycling)
        XCTAssertEqual(panel.level, .normal)
        XCTAssertTrue(panel.hidesOnDeactivate)
        XCTAssertEqual(panel.frame.size, NSSize(width: 660, height: 560))
        model.query = "search only"
        model.pinsOnly = true
        model.cycle()
        XCTAssertFalse(model.browsing)
        XCTAssertTrue(model.query.isEmpty)
        XCTAssertFalse(model.pinsOnly)
        XCTAssertEqual(panel.level, .popUpMenu)
        XCTAssertFalse(panel.hidesOnDeactivate)
        XCTAssertEqual(panel.frame.size, NSSize(width: 480, height: 300))
    }

    @MainActor
    func testLiveReleasePastesIntoTextEdit() async throws {
        guard ProcessInfo.processInfo.environment["HEY_CLIPBOARD_PASTE_TEST"] == "1" else {
            throw XCTSkip("Requires an empty TextEdit document, Hey Talk, and Accessibility")
        }
        previousApp = NSWorkspace.shared.frontmostApplication
        let destination = try XCTUnwrap(NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.TextEdit").first)
        destination.activate()
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertTrue(destination.isActive)
        let application = AXUIElementCreateApplication(destination.processIdentifier)
        var value: CFTypeRef?
        XCTAssertEqual(AXUIElementCopyAttributeValue(application, kAXFocusedUIElementAttribute as CFString, &value), .success)
        let field = try XCTUnwrap(value) as! AXUIElement
        func contents() -> String? {
            var value: CFTypeRef?
            AXUIElementCopyAttributeValue(field, kAXValueAttribute as CFString, &value)
            return value as? String
        }
        guard contents() == "" else { return XCTFail("Focus an empty scratch document; existing text is never replaced") }
        let paused = try Server.request(["command": "status"])["paused"] as? Bool == true
        if !paused { _ = try Server.request(["command": "pause"]) }
        let board = NSPasteboard.general
        let original = (board.pasteboardItems ?? []).map { item in
            Dictionary(uniqueKeysWithValues: item.types.compactMap { type in item.data(forType: type).map { (type, $0) } })
        }
        let text = "Clipboard release verification"
        defer {
            if board.string(forType: .string) == text {
                board.clearContents()
                let items = original.map { values in
                    let item = NSPasteboardItem()
                    for (type, data) in values { item.setData(data, forType: type) }
                    return item
                }
                board.writeObjects(items)
            }
            if !paused { _ = try? Server.request(["command": "resume"]) }
        }
        history.add(text, app: "Test")
        model.cycle()
        let panel = try XCTUnwrap(NSApp.windows.first { $0 is PickerPanel && $0.isVisible })
        // Simulate holding the shortcut while the panel acquires keyboard focus.
        // Physical modifier polling is disabled only for this test interval.
        model.cycling = false
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertTrue(panel.isKeyWindow, "The picker must own keyboard focus before release")
        model.cycling = true
        model.flagsChanged([])
        for _ in 0..<30 {
            if contents() == text || model.message != nil { break }
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTAssertNil(model.message)
        XCTAssertFalse(model.cycling)
        XCTAssertFalse(panel.isVisible)
        XCTAssertEqual(contents(), text)
        if contents() == text { AXUIElementSetAttributeValue(field, kAXValueAttribute as CFString, "" as CFString) }
    }
}
