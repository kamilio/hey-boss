import XCTest
@testable import HeyTalk

final class SelectionCaptureTests: XCTestCase {
    func testDictationPreservesExactSelectionsAfterCorrectionAndWithoutCorrection() {
        let selections = [
            SelectionCapture.Selection(time: Date(), source: "Safari · Address bar", text: "https://example.com/a?q=b&c=d"),
            SelectionCapture.Selection(time: Date(), source: "TextEdit", text: "first\nsecond [1] ```")
        ]
        let expected = "Check these [1] [2]\n\n[1] Safari · Address bar\nhttps://example.com/a?q=b&c=d\n\n[2] TextEdit\nfirst\nsecond [1] ```"
        XCTAssertEqual(SelectionCapture.Selection.attaching(selections, to: .paste("Check these")), .paste(expected))
        XCTAssertEqual(SelectionCapture.Selection.attaching(selections, to: .newThread(project: "test", text: "Check these")), .newThread(project: "test", text: expected))
        XCTAssertEqual(SelectionCapture.Selection.attaching([], to: .paste("Unchanged")), .paste("Unchanged"))
        XCTAssertEqual(SelectionCapture.Selection.attaching(selections, to: .rename("Title")), .rename("Title"))
        XCTAssertEqual(SelectionCapture.Selection.attaching(selections, to: .clearInput), .clearInput)
    }

    func testBrowserAddressFieldsAreDistinctFromOtherSearchFields() {
        XCTAssertTrue(SelectionCapture.isAddressField(identifier: "WEB_BROWSER_ADDRESS_AND_SEARCH_FIELD", domID: "", description: ""))
        XCTAssertTrue(SelectionCapture.isAddressField(identifier: "", domID: "view_1012", description: "Address and search bar"))
        XCTAssertTrue(SelectionCapture.isAddressField(identifier: "", domID: "urlbar-input", description: ""))
        XCTAssertFalse(SelectionCapture.isAddressField(identifier: "", domID: "", description: "Search bookmarks"))
    }

    func testFullSelectionAndSourceAreLiteralInStandardNotice() {
        let text = "    func foo() {\n\n```\n![image](https://invalid.test/x)"
        let notice = SelectionCapture.notice(app: "cmux", detail: "10:01 · devbox", text: text)
        XCTAssertEqual(notice["command"] as? String, "selection_notice")
        XCTAssertEqual(notice["selection_text"] as? String, text)
        XCTAssertEqual(notice["selection_source"] as? String, "cmux · 10:01 · devbox")
        XCTAssertEqual(notice["title"] as? String, "cmux")
        XCTAssertEqual(notice["question"] as? String, "````\n10:01 · devbox\n\n" + text + "\n````")
        XCTAssertEqual(notice["sync"] as? Bool, false)
    }
}
