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
        XCTAssertEqual(capture(root), "## Visible text\nVisible product name\nVisible link")
    }
    func testNestedScrollAreaClipsLoadedButOffscreenText() {
        let scroll = Node("AXScrollArea", frame: CGRect(x: 0, y: 0, width: 300, height: 100), children: [
            Node("AXStaticText", "on screen"),
            Node("AXStaticText", "loaded but scrolled away", frame: CGRect(x: 0, y: 120, width: 100, height: 20))
        ])
        XCTAssertEqual(capture(Node("AXWindow", frame: window, children: [scroll])), "## Visible text\non screen")
    }
    func testVirtualMessageWrappersDoNotHideVisibleMessages() {
        let root = Node("AXWindow", frame: window, children: [
            Node("AXGroup", frame: .zero, children: [
                Node("AXList", frame: CGRect(x: 0, y: 0, width: 1, height: 1), children: [
                    Node("AXStaticText", "visible message"),
                    Node("AXStaticText", "offscreen message", frame: CGRect(x: 0, y: 600, width: 100, height: 20)),
                    Node("AXStaticText", "clamped message", frame: CGRect(x: 0, y: 499, width: 100, height: 1))
                ])
            ])
        ])
        XCTAssertEqual(capture(root), "## Visible text\nvisible message")
    }
    func testChromiumClampedOffscreenNodesAndGenericScrollGroupsAreExcluded() {
        let root = Node("AXWindow", frame: window, children: [
            Node("AXGroup", frame: CGRect(x: 0, y: 0, width: 300, height: 100), children: [
                Node("AXStaticText", "visible"),
                Node("AXStaticText", "clipped below group", frame: CGRect(x: 10, y: 120, width: 100, height: 20)),
                Node("AXStaticText", "clamped offscreen", frame: CGRect(x: 10, y: 99, width: 100, height: 1))
            ])
        ])
        XCTAssertEqual(capture(root), "## Visible text\nvisible")
    }
    func testNamedRegionsAndMessageHeadingsPreserveTheirSources() {
        let sidebar = Node("AXGroup", children: [Node("AXStaticText", "Project")])
        sidebar.attributes["AXDescription"] = "Sidebar"
        let conversation = Node("AXGroup", children: [
            Node("AXHeading", children: [Node("AXStaticText", "You")]),
            Node("AXStaticText", "Use the existing library."),
            Node("AXHeading", children: [Node("AXStaticText", "Assistant")]),
            Node("AXStaticText", "The library is named ExampleKit.")
        ])
        conversation.attributes["AXDescription"] = "Conversation"
        XCTAssertEqual(capture(Node("AXWindow", frame: window, children: [sidebar, conversation, Node("AXStaticText", "Footer")])),
                       "## Sidebar\nProject\n\n## Conversation / You\nUse the existing library.\n\n## Conversation / Assistant\nThe library is named ExampleKit.\n\n## Visible text\nFooter")
    }
    func testTerminalUsesOnlyExplicitVisibleTextAndStillRejectsEditors() {
        let terminal = Node("AXTextArea", "full scrollback")
        terminal.attributes["AXVisibleText"] = "visible terminal output"
        terminal.attributes["AXEditable"] = true
        let editor = Node("AXTextArea", "draft")
        XCTAssertEqual(capture(Node("AXWindow", frame: window, children: [Node("AXStaticText", "workspace chrome"), terminal, editor])),
                       "## Terminal\nvisible terminal output")
        terminal.attributes["AXFrame"] = CGRect(x: 0, y: 600, width: 100, height: 20)
        XCTAssertNil(capture(Node("AXWindow", frame: window, children: [terminal])))
    }
    func testConversationCaptureKeepsSpeakerMetadataAndCurrentInputButSkipsSidebar() {
        let speaker = Node("AXHeading", frame: CGRect(x: 0, y: 0, width: 1, height: 1), children: [Node("AXStaticText", "You")])
        speaker.attributes["AXHidden"] = true
        let sidebar = Node("AXGroup", children: [Node("AXStaticText", "laptop-name")])
        sidebar.attributes["AXDescription"] = "Thread details"
        let input = Node("AXTextArea", "Existing draft")
        input.attributes["AXDescription"] = "Message"
        input.attributes["AXTitle"] = ""
        input.attributes["AXEditable"] = true
        let root = Node("AXWindow", frame: window, children: [
            sidebar, Node("AXGroup", children: [speaker, Node("AXStaticText", "What is ExampleKit?")]),
            Node("AXStaticText", "status chrome"), Node("AXGroup", frame: CGRect(x: 14, y: 14, width: 92, height: 12), children: [input])
        ])
        XCTAssertEqual(AppContext.visibleText(in: root, viewport: window, deadline: ProcessInfo.processInfo.systemUptime + 1, conversationOnly: true) { $0.attributes[$1] },
                       "## Conversation / You\nWhat is ExampleKit?\n\n## Current input\nExisting draft")
    }
    func testCurrentInputExcludesPasswordsAndUnfocusedEditors() {
        let focused = Node("AXTextField", "search words")
        focused.attributes["AXFocused"] = true
        let password = Node("AXTextField", "secret")
        password.attributes["AXFocused"] = true
        password.attributes["AXSubrole"] = "AXSecureTextField"
        let outline = Node("AXOutline", children: [Node("AXStaticText", "unrelated channel")])
        XCTAssertEqual(capture(Node("AXWindow", frame: window, children: [focused, password, outline, Node("AXTextArea", "unfocused editor")])),
                       "## Current input\nsearch words")
    }
    func testSlackMessageListIsCapturedWithoutNavigationChrome() {
        let messages = Node("AXList", frame: .zero, children: [Node("AXStaticText", "Please use ExampleKit.")])
        messages.attributes["AXTitle"] = ""
        messages.attributes["AXDescription"] = "Example (channel)"
        let root = Node("AXWindow", frame: window, children: [Node("AXStaticText", "Messages"), messages])
        XCTAssertEqual(AppContext.visibleText(in: root, viewport: window, deadline: ProcessInfo.processInfo.systemUptime + 1, conversationOnly: true) { $0.attributes[$1] },
                       "## Conversation / Example (channel)\nPlease use ExampleKit.")
    }
    func testContextIsBoundedAndExpiredCaptureReturnsNothing() {
        let root = Node("AXWindow", frame: window, children: [Node("AXStaticText", String(repeating: "x", count: 9000)), Node("AXStaticText", "last visible words")])
        let text = capture(root)
        XCTAssertEqual(text?.count, 8000)
        XCTAssertTrue(text?.hasSuffix("last visible words") == true)
        XCTAssertNil(AppContext.visibleText(in: root, viewport: window, deadline: 0) { $0.attributes[$1] })
    }
}
