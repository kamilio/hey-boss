import AppKit
import XCTest
@testable import HeyTalk

final class SpeechPassageTests: XCTestCase {
    private struct Block {
        let name: String
        let role: String
        var children: [Block] = []
    }
    private func following(_ name: String, in root: Block) -> [String] {
        var parents: [String: Block] = [:], nodes: [String: Block] = [:]
        func index(_ block: Block) {
            nodes[block.name] = block
            for child in block.children { parents[child.name] = block; index(child) }
        }
        index(root)
        return SpeechPassageStructure.following(after: nodes[name]!, parent: { parents[$0.name] },
            role: { $0.role }, subrole: { _ in "" }, children: { $0.children }, same: { $0.name == $1.name }).map(\.name)
    }
    func testReadingFromInsideAListContinuesIntoTheFollowingParagraphsButNotTheNextMessage() {
        let first = Block(name: "first", role: "AXParagraph", children: [Block(name: "first-text", role: "AXStaticText")])
        let second = Block(name: "second", role: "AXParagraph", children: [Block(name: "second-text", role: "AXStaticText")])
        let item = Block(name: "item", role: "AXListItem", children: [Block(name: "bullet", role: "AXListMarker"), first, second])
        let list = Block(name: "list", role: "AXList", children: [item])
        let after = Block(name: "after", role: "AXParagraph", children: [Block(name: "after-text", role: "AXStaticText")])
        let message = Block(name: "message", role: "AXGroup", children: [list, Block(name: "spacer", role: "AXGroup"), after, Block(name: "copy", role: "AXButton")])
        let thread = Block(name: "thread", role: "AXGroup", children: [message, Block(name: "unrelated", role: "AXStaticText")])
        XCTAssertEqual(following("first", in: thread), ["second", "after"])
        XCTAssertEqual(following("second", in: thread), ["after"])
        XCTAssertEqual(following("after", in: thread), [])
    }
    func testNestedSectionsContinueInOrderAndAControlBeforeTheStartStillPreventsLeavingTheMessage() {
        let a = Block(name: "a", role: "AXStaticText"), b = Block(name: "b", role: "AXStaticText")
        let nested = Block(name: "nested", role: "AXGroup", children: [a, Block(name: "spacer", role: "AXGroup"), b])
        let message = Block(name: "message", role: "AXGroup", children: [Block(name: "menu", role: "AXButton"), nested, Block(name: "c", role: "AXStaticText")])
        let root = Block(name: "root", role: "AXGroup", children: [message, Block(name: "other-message", role: "AXStaticText")])
        XCTAssertEqual(following("a", in: root), ["b", "c"])
    }
    func testHeadedMessageBodyDoesNotReadItsSiblingTimestampOrNextMessage() {
        let body = Block(name: "body", role: "AXGroup", children: [
            Block(name: "speaker", role: "AXHeading", children: [Block(name: "name", role: "AXStaticText")]),
            Block(name: "first", role: "AXStaticText"), Block(name: "second", role: "AXStaticText")])
        let message = Block(name: "message", role: "AXGroup", children: [body,
            Block(name: "timestamp", role: "AXStaticText"), Block(name: "copy", role: "AXButton")])
        XCTAssertEqual(following("first", in: message), ["second"])
    }
    func testNestedParagraphsAndListsFormOneGroupButControlsStopTraversal() throws {
        let a = Block(name: "first", role: "AXGroup", children: [Block(name: "text", role: "AXStaticText")])
        let b = Block(name: "second", role: "AXParagraph", children: [Block(name: "text", role: "AXStaticText")])
        let list = Block(name: "list", role: "AXList", children: [Block(name: "item", role: "AXListItem", children: [Block(name: "bullet", role: "AXListMarker"), Block(name: "text", role: "AXStaticText")])])
        let section = Block(name: "section", role: "AXGroup", children: [a, Block(name: "wrapper", role: "AXGroup", children: [b, list])])
        func blocks(_ node: Block) -> [Block]? { SpeechPassageStructure.blocks(in: node, role: { $0.role }, subrole: { _ in "" }, children: { $0.children }) }
        XCTAssertEqual(try XCTUnwrap(blocks(section)).map(\.name), ["first", "second", "item"])
        let controls = Block(name: "message", role: "AXGroup", children: [section, Block(name: "copy", role: "AXButton")])
        XCTAssertNil(blocks(controls), "A message with controls must be a boundary, not a readable wrapper")
        XCTAssertNil(blocks(Block(name: "window", role: "AXWebArea", children: [section])))
    }
    func testContinuationPreservesParagraphRangesAndRichTextWithUnicode() throws {
        let first = SpeechTarget(text: "Hello 🌍.", frame: CGRect(x: 10, y: 10, width: 200, height: 30), point: CGPoint(x: 20, y: 20), pid: 1, bundleID: "test", source: "paragraph")
        var second = first
        second = SpeechTarget(text: "Next paragraph.", frame: CGRect(x: 10, y: 50, width: 200, height: 30), point: first.point, pid: 1, bundleID: "test", source: "paragraph", attributed: NSAttributedString(string: "Next paragraph.", attributes: [.font: NSFont.boldSystemFont(ofSize: 16)]))
        let joined = SpeechPassage.join([first, second])
        XCTAssertEqual(joined.text, "Hello 🌍.\nNext paragraph.", "Paragraph spacing must not include an extra blank paragraph")
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
