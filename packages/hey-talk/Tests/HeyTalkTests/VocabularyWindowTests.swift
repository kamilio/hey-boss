import AppKit
import XCTest
@testable import HeyTalk

final class VocabularyWindowTests: XCTestCase {
    @MainActor func testTrashImmediatelyRemovesOnlyTheChosenWord() async throws {
        let app = NSApplication.shared, editor = VocabularyWindow()
        var words = ["Atlas", "Koonal Tum"]
        var removes = 0
        editor.show(term: nil) { args, reply in
            if !args.isEmpty {
                XCTAssertEqual(args, ["remove", "Koonal Tum"])
                removes += 1; words.removeLast()
            }
            reply(["ok": true, "vocabulary": words])
        }
        let window = try XCTUnwrap(app.windows.first { $0.title == "Hey Talk — Vocabulary" && $0.isVisible })
        defer { window.close() }
        try await Task.sleep(for: .milliseconds(400))
        let root = try XCTUnwrap(window.contentView)
        func views(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(views) }
        let scroll = try XCTUnwrap(views(root).compactMap { $0 as? NSScrollView }.first { $0.documentView != nil && $0.frame.width > 400 })
        let frame = scroll.convert(scroll.bounds, to: nil)
        // Click the trailing trash button in the second row of this two-entry fixture.
        let point = NSPoint(x: frame.maxX - 42, y: frame.maxY - 92)
        for type: NSEvent.EventType in [.leftMouseDown, .leftMouseUp] {
            window.sendEvent(try XCTUnwrap(NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber, context: nil, eventNumber: 1, clickCount: 1, pressure: 1)))
        }
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertNil(window.attachedSheet)
        XCTAssertEqual(removes, 1)
        XCTAssertEqual(words, ["Atlas"])
    }

    @MainActor func testPencilOpensSavedSpellingAndReturnSubmitsTheCorrection() async throws {
        let app = NSApplication.shared
        let overlay = Overlay(), editor = VocabularyWindow()
        overlay.show(true)
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(app.windows.last { $0.title == "Hey Talk — Recording" })
        let window = try XCTUnwrap(app.windows.last { $0.title == "Hey Talk — Vocabulary" })
        defer { window.close() }
        func views(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(views) }
        let saved = expectation(description: "Save corrected spelling")
        overlay.onEditVocabulary = { term in
            XCTAssertEqual(term, "Koonal Tum")
            editor.show(term: term) { args, reply in
                if args.isEmpty { reply(["ok": true, "vocabulary": ["Atlas", "Koonal Tum"]]); return }
                XCTAssertEqual(args, ["rename", "Koonal Tum", "Kunal Tum"])
                reply(["ok": true, "vocabulary": ["Atlas", "Kunal Tum"]])
                saved.fulfill()
            }
        }
        overlay.confirmAction(.addVocabularyTerm("Koonal Tum"))
        let pill = views(try XCTUnwrap(panel.contentView))
        XCTAssertTrue(pill.contains { ($0 as? NSTextField)?.stringValue == "Added Koonal Tum" && !$0.isHidden })
        let pencil = try XCTUnwrap(pill.compactMap { $0 as? NSButton }.first { $0.accessibilityLabel() == "Edit vocabulary" })
        XCTAssertFalse(pencil.isHidden)
        pencil.performClick(nil)
        XCTAssertTrue(window.isVisible)
        try await Task.sleep(for: .milliseconds(250))
        let field = try XCTUnwrap(views(try XCTUnwrap(window.contentView)).compactMap { $0 as? NSTextField }.first { $0.isEditable && $0.stringValue == "Koonal Tum" })
        field.selectText(nil)
        let input = try XCTUnwrap(field.currentEditor())
        input.insertText("Kunal Tum")
        input.doCommand(by: #selector(NSResponder.insertNewline(_:)))
        await fulfillment(of: [saved], timeout: 2)
    }
}
