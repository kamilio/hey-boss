import AppKit
import XCTest
@testable import HeyTalk

final class VocabularyWindowTests: XCTestCase {
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
