import XCTest
@testable import HeyTalk

final class SelectionCaptureTests: XCTestCase {
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
