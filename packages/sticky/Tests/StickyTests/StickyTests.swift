import XCTest
import AppKit
@testable import Sticky

final class StickyTests: XCTestCase {
    @MainActor func testNoteShowsAndHidesWithoutTakingKeyboardFocus() throws {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let previous = NSWorkspace.shared.frontmostApplication?.processIdentifier
        let note = try NoteWindow(note: Note(id: "passive-test", markdown: "# Focus\n\nA passive note."), position: 0, save: { _ in })
        defer {
            note.invalidate()
            for key in ["frame", "frames", "screen"] { UserDefaults.standard.removeObject(forKey: "\(key).passive-test") }
        }
        note.show()
        RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        XCTAssertTrue(note.window.isVisible)
        XCTAssertFalse(note.window.canBecomeKey)
        XCTAssertFalse(note.window.canBecomeMain)
        XCTAssertFalse(note.window.isKeyWindow)
        XCTAssertEqual(app.activationPolicy(), .accessory)
        XCTAssertEqual(NSWorkspace.shared.frontmostApplication?.processIdentifier, previous)
        let surface = try XCTUnwrap(note.window.contentView as? NSGlassEffectView)
        let handle = try XCTUnwrap(surface.contentView?.subviews.last)
        note.window.setFrameOrigin(NSPoint(x: note.window.frame.minX - 100, y: note.window.frame.minY))
        let start = NSPoint(x: 384, y: 12)
        XCTAssertTrue(surface.hitTest(start) === handle)
        let down = try XCTUnwrap(NSEvent.mouseEvent(with: .leftMouseDown, location: start, modifierFlags: [], timestamp: 0, windowNumber: note.window.windowNumber, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
        let drag = try XCTUnwrap(NSEvent.mouseEvent(with: .leftMouseDragged, location: NSPoint(x: 424, y: -88), modifierFlags: [], timestamp: 0.1, windowNumber: note.window.windowNumber, context: nil, eventNumber: 2, clickCount: 1, pressure: 1))
        handle.mouseDown(with: down); handle.mouseDragged(with: drag); handle.mouseUp(with: drag)
        XCTAssertEqual(note.window.frame.size, NSSize(width: 440, height: 520))
        XCTAssertFalse(note.window.isKeyWindow)
        let savedFrame = note.window.frame
        note.window.setFrameOrigin(NSPoint(x: savedFrame.minX - 50, y: savedFrame.minY))
        NotificationCenter.default.post(name: NSApplication.didChangeScreenParametersNotification, object: app)
        RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        XCTAssertEqual(note.window.frame, savedFrame, "Automatic display moves must not overwrite the user's placement")
        note.hideNote()
        XCTAssertFalse(note.window.isVisible)
        XCTAssertEqual(NSWorkspace.shared.frontmostApplication?.processIdentifier, previous)
    }
    func testPrivatePersistenceAndInvalidIDs() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent(".build/store-test-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = try NoteStore(directory: root)
        try store.ensureDefault()
        XCTAssertEqual(try store.all().map(\.id), ["focus"])
        let text = "# Today's **focus**\n\n1. [Plan](https://example.com)\n2. Ship 🚀\n"
        try store.save(Note(id: "focus", markdown: text))
        try store.ensureDefault()
        XCTAssertEqual(try store.read("focus").markdown, text)
        XCTAssertEqual(try store.read("focus").title, "Today's focus")
        XCTAssertEqual(try FileManager.default.attributesOfItem(atPath: store.url("focus").path)[.posixPermissions] as? Int, 0o600)
        for id in ["../outside", "/absolute", "", "UPPER", "a/b", "note.md"] { XCTAssertThrowsError(try store.save(Note(id: id, markdown: "bad"))) }
        try store.save(Note(id: "second", markdown: "# Second"))
        XCTAssertEqual(try store.all().count, 2)
        XCTAssertThrowsError(try store.save(Note(id: "focus", markdown: "bad\0source")))
        XCTAssertEqual(try store.read("focus").markdown, text)
    }
    func testCLIRejectsInvalidExportsAndExtraArguments() throws {
        for args in [["preview","a.md"], ["preview","a.md","--output","note.md"], ["export","--output","a.png","--width","nan"], ["export","--output","a.png","--width","2000"], ["set"], ["show","unexpected"], ["get","--id","a","--id","b"]] {
            XCTAssertThrowsError(try CLI(args), args.joined(separator: " "))
        }
        let cli = try CLI(["preview","-","--output","note.png","--appearance","dark"])
        XCTAssertEqual(cli.file, "-")
        XCTAssertEqual(cli.options["appearance"], "dark")
    }
    func testTitleIsExtractedByParserAndBodyIsSafe() throws {
        let doc = try RenderedDocument("```\n# Not the title\n```\n\n# Daily **Focus**\n\n[Plan](https://example.com)\n\n<script>alert(1)</script>")
        XCTAssertEqual(doc.title, "Daily Focus")
        XCTAssertTrue(doc.html.contains("https://example.com"))
        XCTAssertFalse(doc.html.contains("<script>"))
        XCTAssertFalse(doc.html.contains("<h1>"))
    }
}
