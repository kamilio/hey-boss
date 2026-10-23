import AppKit
import XCTest
@testable import HeyTalk

final class AppContextTests: XCTestCase {
    private final class Node {
        var attributes: [String: Any]
        init(_ role: String, _ text: String = "", frame: CGRect = CGRect(x: 10, y: 10, width: 100, height: 20), children: [Node] = []) {
            attributes = ["AXRole": role, "AXValue": text, "AXFrame": frame, "AXChildren": children]
        }
    }
    private let window = CGRect(x: 0, y: 0, width: 500, height: 500)
    private func capture(_ root: Node) -> String? {
        AppContext.visibleText(in: root, viewport: window, deadline: ProcessInfo.processInfo.systemUptime + 1) { $0.attributes[$1] }
    }
    func testOnlyVisibleReadOnlyTextIsIncluded() {
        let secret = Node("AXTextField", "password", children: [Node("AXStaticText", "nested password")])
        secret.attributes["AXSubrole"] = "AXSecureTextField"
        let hidden = Node("AXStaticText", "hidden")
        hidden.attributes["AXHidden"] = true
        let editor = Node("AXGroup", children: [Node("AXStaticText", "draft")])
        editor.attributes["AXEditable"] = true
        let root = Node("AXWindow", frame: window, children: [
            Node("AXStaticText", "Visible product name"), secret, hidden, editor,
            Node("AXTextArea", "draft", children: [Node("AXStaticText", "nested draft")]),
            Node("AXButton", "button", children: [Node("AXStaticText", "button label")]),
            Node("AXStaticText", "outside", frame: CGRect(x: 0, y: 600, width: 100, height: 20)),
            Node("AXStaticText", "partly outside", frame: CGRect(x: 0, y: 490, width: 100, height: 30)),
            Node("AXLink", frame: CGRect(x: 10, y: 50, width: 200, height: 30), children: [
                Node("AXStaticText", "Visible link", frame: CGRect(x: 10, y: 50, width: 100, height: 20))
            ])
        ])
        XCTAssertEqual(capture(root), "Visible product name\nVisible link")
    }
    func testNestedScrollAreaClipsLoadedButOffscreenText() {
        let scroll = Node("AXScrollArea", frame: CGRect(x: 0, y: 0, width: 300, height: 100), children: [
            Node("AXStaticText", "on screen"),
            Node("AXStaticText", "loaded but scrolled away", frame: CGRect(x: 0, y: 120, width: 100, height: 20))
        ])
        XCTAssertEqual(capture(Node("AXWindow", frame: window, children: [scroll])), "on screen")
    }
    func testChromiumClampedOffscreenNodesAndGenericScrollGroupsAreExcluded() {
        let root = Node("AXWindow", frame: window, children: [
            Node("AXGroup", frame: CGRect(x: 0, y: 0, width: 300, height: 100), children: [
                Node("AXStaticText", "visible"),
                Node("AXStaticText", "clipped below group", frame: CGRect(x: 10, y: 120, width: 100, height: 20)),
                Node("AXStaticText", "clamped offscreen", frame: CGRect(x: 10, y: 99, width: 100, height: 1))
            ])
        ])
        XCTAssertEqual(capture(root), "visible")
    }
    func testContextIsBoundedAndExpiredCaptureReturnsNothing() {
        let root = Node("AXWindow", frame: window, children: [Node("AXStaticText", String(repeating: "x", count: 9000)), Node("AXStaticText", "last visible words")])
        let text = capture(root)
        XCTAssertEqual(text?.count, 8000)
        XCTAssertTrue(text?.hasSuffix("last visible words") == true)
        XCTAssertNil(AppContext.visibleText(in: root, viewport: window, deadline: 0) { $0.attributes[$1] })
    }
}
