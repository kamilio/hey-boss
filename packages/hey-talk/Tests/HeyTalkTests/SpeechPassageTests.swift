import AppKit
import XCTest
@testable import HeyTalk

final class SpeechPassageTests: XCTestCase {
    func testContinuationPreservesParagraphRangesAndRichTextWithUnicode() throws {
        let first = SpeechTarget(text: "Hello 🌍.", frame: CGRect(x: 10, y: 10, width: 200, height: 30), point: CGPoint(x: 20, y: 20), pid: 1, bundleID: "test", source: "paragraph")
        var second = first
        second = SpeechTarget(text: "Next paragraph.", frame: CGRect(x: 10, y: 50, width: 200, height: 30), point: first.point, pid: 1, bundleID: "test", source: "paragraph", attributed: NSAttributedString(string: "Next paragraph.", attributes: [.font: NSFont.boldSystemFont(ofSize: 16)]))
        let joined = SpeechPassage.join([first, second])
        XCTAssertEqual(joined.text, "Hello 🌍.\n\nNext paragraph.")
        XCTAssertEqual(joined.frame, first.frame, "The control stays anchored to the starting paragraph")
        XCTAssertEqual(joined.paragraphs.count, 2)
        let range = joined.paragraphs[1].range
        XCTAssertEqual((joined.text as NSString).substring(with: range), second.text)
        XCTAssertEqual(joined.attributed?.attribute(.font, at: range.location, effectiveRange: nil) as? NSFont, .boldSystemFont(ofSize: 16))
        XCTAssertEqual(joined.paragraph(containing: range.location)?.frame, second.frame)
        XCTAssertNil(joined.paragraph(containing: (joined.text as NSString).length))
    }
    func testContinuationStopsBeforeCharacterBudgetWithoutCuttingParagraphs() {
        let first = SpeechTarget(text: String(repeating: "A", count: 15_000), frame: .zero, point: .zero, pid: 1, bundleID: "test", source: "paragraph")
        XCTAssertEqual(SpeechPassage.join([first, first]).paragraphs.count, 1)
    }
    func testSourceHighlightClipsToVisibleViewport() {
        let viewport = CGRect(x: 100, y: 100, width: 600, height: 400)
        XCTAssertEqual(SpeechSourceHighlight.clipped(CGRect(x: 120, y: 60, width: 500, height: 100), to: viewport), CGRect(x: 116, y: 100, width: 508, height: 64))
        XCTAssertNil(SpeechSourceHighlight.clipped(CGRect(x: 120, y: 600, width: 500, height: 30), to: viewport))
    }
}
