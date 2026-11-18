import AppKit
import XCTest
@testable import HeyClipboard

final class CycleTests: XCTestCase {
    private var directory: URL!
    private var history: History!
    private var model: ClipboardApp!

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
    func testReleasingEitherModifierEndsCycleOnlyOnce() {
        // Empty history exercises release without touching the real clipboard.
        for remaining: NSEvent.ModifierFlags in [.command, .shift, []] {
            model.cycle()
            XCTAssertTrue(model.cycling)
            model.flagsChanged(remaining)
            XCTAssertFalse(model.cycling)
            model.message = "already handled"
            model.flagsChanged([])
            XCTAssertEqual(model.message, "already handled")
            model.dismiss()
        }
    }
    func testEscapeCancelsWhileModifiersRemainHeld() throws {
        history.add("must not paste", app: "Test")
        let count = NSPasteboard.general.changeCount
        model.cycle()
        let panel = try XCTUnwrap(NSApp.windows.first { $0 is PickerPanel })
        let escape = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero,
            modifierFlags: [.command, .shift], timestamp: 0, windowNumber: panel.windowNumber,
            context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}",
            isARepeat: false, keyCode: 53))
        panel.sendEvent(escape)
        XCTAssertFalse(model.cycling)
        XCTAssertFalse(panel.isVisible)
        model.flagsChanged([])
        XCTAssertEqual(NSPasteboard.general.changeCount, count)
    }
}
