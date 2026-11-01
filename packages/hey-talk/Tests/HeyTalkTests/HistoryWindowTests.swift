import AppKit
import TalkCore
import XCTest
@testable import HeyTalk

final class HistoryWindowTests: XCTestCase {
    @MainActor func testFirstLayoutShowsSelectablePreviewsAndKeepsTheFullTranscript() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = try HistoryStore(directory: directory)
        for i in 0..<49 {
            try store.append(Dictation(text: "Earlier dictation \(i)", originalText: "", audioSeconds: 1, source: "file"))
        }
        let text = String(repeating: "Words from a long dictation. ", count: 800)
        let entry = Dictation(text: text, originalText: "Um, " + text, audioSeconds: 600, source: "microphone")
        try store.append(entry)
        let history = DictationHistory(directory: directory)
        history.refresh()
        let loaded = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in !history.loading }, object: nil)
        wait(for: [loaded], timeout: 5)

        let app = NSApplication.shared
        let existing = Set(app.windows.map(ObjectIdentifier.init))
        let controller = HistoryWindow(history: history)
        defer { withExtendedLifetime(controller) {} }
        let window = try XCTUnwrap(app.windows.first { !existing.contains(ObjectIdentifier($0)) && $0.title == "Hey Talk — Dictation History" })
        let root = try XCTUnwrap(window.contentView)
        root.layoutSubtreeIfNeeded()
        func fields(in view: NSView) -> [NSTextField] {
            (view as? NSTextField).map { [$0] } ?? view.subviews.flatMap(fields)
        }
        let labels = fields(in: root)
        let preview = try XCTUnwrap(labels.first { $0.stringValue.hasPrefix("Words from a long dictation.") })
        XCTAssertTrue(preview.isSelectable)
        XCTAssertTrue(preview.stringValue.hasSuffix("…"))
        XCTAssertLessThan(preview.stringValue.count, text.count)
        XCTAssertFalse(labels.contains { $0.stringValue == "Earlier dictation 0" }, "Offscreen entries should not be laid out on open")
        XCTAssertEqual(history.entries.first, entry)
        XCTAssertFalse(window.isVisible)
    }
}
