import AppKit
import XCTest
@testable import HeyTalk

final class SpeechOverlayTests: XCTestCase {
    func testPlaceholderOverAnEditorIsExcludedButAdjacentParagraphIsReadable() {
        struct Node {
            var frame: CGRect
            var interactive = false
            var children: [Node] = []
        }
        let editor = Node(frame: CGRect(x: 100, y: 300, width: 700, height: 80), interactive: true)
        let placeholder = Node(frame: CGRect(x: 104, y: 304, width: 692, height: 72))
        let paragraph = Node(frame: CGRect(x: 100, y: 200, width: 700, height: 60))
        let wrapper = Node(frame: editor.frame, children: [editor, placeholder])
        let section = Node(frame: CGRect(x: 100, y: 200, width: 700, height: 180), children: [paragraph, wrapper])
        func excluded(_ ancestors: [Node], _ point: CGPoint) -> Bool {
            SpeechPassageStructure.interactiveRegion(ancestors: ancestors, point: point,
                frame: { $0.frame }, interactive: { $0.interactive }, children: { $0.children })
        }
        XCTAssertTrue(excluded([placeholder, wrapper, section], CGPoint(x: 120, y: 310)))
        XCTAssertTrue(excluded([placeholder, wrapper, section], CGPoint(x: 450, y: 350)), "Empty editor space is still interactive")
        XCTAssertFalse(excluded([paragraph, section], CGPoint(x: 450, y: 230)))
    }
    func testInlineStylesBelongToTheContainingPassageButContainersAndControlsDoNot() {
        for (role, subrole) in [("AXStaticText", ""), ("AXLink", ""), ("AXGroup", "AXStrongStyleGroup"), ("AXGroup", "AXEmphasisStyleGroup"), ("AXGroup", "AXCodeStyleGroup")] {
            XCTAssertTrue(SpeechPassageStructure.inline(role: role, subrole: subrole))
        }
        for role in ["AXGroup", "AXWebArea", "AXButton", "AXTextArea", "AXHeading", "AXList"] {
            XCTAssertFalse(SpeechPassageStructure.inline(role: role, subrole: ""))
        }
    }
    func testClippedParagraphControlsStayOnScreenIncludingSecondaryDisplay() {
        for visible in [CGRect(x: 0, y: 25, width: 1440, height: 875), CGRect(x: -1920, y: -300, width: 1920, height: 1080)] {
            for anchor in [CGRect(x: visible.minX + 400, y: visible.maxY - 30, width: 600, height: 220), CGRect(x: visible.minX + 400, y: visible.minY - 180, width: 600, height: 220)] {
                let frame = SpeechOverlay.placement(anchor: anchor, size: CGSize(width: 264, height: 42), visible: visible)
                XCTAssertTrue(visible.contains(frame), "\(frame) outside \(visible)")
            }
        }
    }
    func testControlIsAnchoredToParagraphAndBridgeKeepsItReachable() {
        let visible = CGRect(x: 0, y: 0, width: 1440, height: 900)
        let anchor = CGRect(x: 200, y: 300, width: 600, height: 200)
        let control = SpeechOverlay.placement(anchor: anchor, size: CGSize(width: 96, height: 36), visible: visible)
        XCTAssertEqual(control.minX, anchor.maxX + 8)
        XCTAssertEqual(control.maxY, anchor.maxY)
        let bridge = SpeechOverlay.hoverRegion(paragraph: anchor, control: control)
        for x in stride(from: anchor.maxX - 20, through: control.midX, by: 2) {
            XCTAssertTrue(bridge.contains(CGPoint(x: x, y: control.midY)), "No dead gap on the way to Listen")
        }
        XCTAssertFalse(bridge.contains(CGPoint(x: 100, y: 100)))
        let tall = SpeechOverlay.placement(anchor: CGRect(x: 200, y: -400, width: 600, height: 1800), size: control.size, visible: visible)
        XCTAssertTrue(visible.contains(tall))
        XCTAssertEqual(tall.minX, control.minX)
    }
    @MainActor func testListenCapsuleFitsItsLabelAndCentersTheIconAndTextTogether() throws {
        let app = NSApplication.shared, overlay = SpeechOverlay()
        defer { overlay.hide() }
        overlay.show(target: SpeechTarget(text: "A real paragraph.", frame: CGRect(x: 100, y: 100, width: 500, height: 50), point: CGPoint(x: 120, y: 120), pid: 1, bundleID: "test", source: "text"), state: .idle, rate: 1, fraction: 0, error: nil)
        let panel = try XCTUnwrap(app.windows.last { $0.title == "Hey Talk — Read Aloud" })
        func descendants(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(descendants) }
        let views = descendants(try XCTUnwrap(panel.contentView))
        let label = try XCTUnwrap(views.compactMap { $0 as? NSTextField }.first { $0.superview is GlassFace })
        let icon = try XCTUnwrap(views.compactMap { $0 as? NSImageView }.first { $0.superview is GlassFace })
        XCTAssertEqual(label.stringValue, "Listen")
        XCTAssertGreaterThanOrEqual(label.frame.width, label.fittingSize.width)
        XCTAssertEqual(icon.frame.union(label.frame).midX, panel.frame.width / 2, accuracy: 1)
        XCTAssertEqual(icon.frame.midY, label.frame.midY, accuracy: 1)
        let face = try XCTUnwrap(label.superview)
        XCTAssertTrue(face.hitTest(CGPoint(x: label.frame.midX, y: label.frame.midY)) is NSButton, "The Listen label must pass clicks to the full capsule button")
        XCTAssertTrue(face.hitTest(CGPoint(x: icon.frame.midX, y: icon.frame.midY)) is NSButton, "The play icon must pass clicks to the full capsule button")
        let glassFace = try XCTUnwrap(face as? GlassFace)
        let event = try XCTUnwrap(NSEvent.mouseEvent(with: .mouseMoved, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: panel.windowNumber, context: nil, eventNumber: 0, clickCount: 0, pressure: 0))
        face.mouseEntered(with: event)
        XCTAssertTrue(glassFace.emphasized, "Hover must visibly brighten the capsule")
        face.mouseExited(with: event)
        XCTAssertFalse(glassFace.emphasized)
    }
    @MainActor func testSourceHoverHighlightPassesClicksThrough() throws {
        let highlight = SpeechSourceHighlight()
        defer { highlight.hide() }
        highlight.show(frame: CGRect(x: 100, y: 100, width: 500, height: 60), viewport: CGRect(x: 0, y: 50, width: 1400, height: 800), reading: false)
        let panel = try XCTUnwrap(NSApplication.shared.windows.last { $0.title == "Hey Talk — Reading passage" })
        XCTAssertTrue(panel.isVisible)
        XCTAssertTrue(panel.ignoresMouseEvents)
        XCTAssertFalse(panel.canBecomeKey)
        XCTAssertFalse(panel.canBecomeMain)
    }
    @MainActor func testDrawerShowsCurrentParagraphInTheGroup() throws {
        let first = SpeechTarget(text: "First paragraph.", frame: CGRect(x: 200, y: 200, width: 600, height: 30), point: CGPoint(x: 220, y: 210), pid: 1, bundleID: "test", source: "paragraph")
        let second = SpeechTarget(text: "Second paragraph.", frame: CGRect(x: 200, y: 250, width: 600, height: 30), point: first.point, pid: 1, bundleID: "test", source: "paragraph")
        let target = SpeechPassage.join([first, second]), overlay = SpeechOverlay()
        defer { overlay.hide() }
        overlay.show(target: target, state: .playing, rate: 1.25, fraction: 0, error: nil)
        overlay.highlight((target.paragraphs[1].range, false))
        let panel = try XCTUnwrap(NSApplication.shared.windows.last { $0.title == "Hey Talk — Read Aloud" })
        func views(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(views) }
        let label = try XCTUnwrap(views(try XCTUnwrap(panel.contentView)).compactMap { $0 as? NSTextField }.first { $0.superview is GlassFace })
        XCTAssertEqual(label.stringValue, "Reading 2 of 2")
        overlay.show(target: target, state: .idle, rate: 1.25, fraction: 0, error: nil)
        XCTAssertEqual(label.stringValue, "Listen · 2 paragraphs")
        XCTAssertGreaterThanOrEqual(label.frame.minX, 18)
        XCTAssertLessThanOrEqual(label.frame.maxX, panel.frame.width - 12, "The paragraph count must fit inside the hover capsule")
    }
    @MainActor func testCapturedFormattingSurvivesHighlightAndDrawerNeverTakesFocus() throws {
        let app = NSApplication.shared
        let raw = NSMutableAttributedString(string: "Plain bold code link.")
        raw.addAttribute(NSAttributedString.Key("AXFont"), value: ["AXFontSize": 14, "AXFontBold": true], range: NSRange(location: 6, length: 4))
        raw.addAttribute(NSAttributedString.Key("AXFont"), value: ["AXFontSize": 13, "AXFontName": "Menlo-Regular"], range: NSRange(location: 11, length: 4))
        raw.addAttribute(NSAttributedString.Key("AXForegroundColor"), value: NSColor.systemBlue.cgColor, range: NSRange(location: 16, length: 4))
        let target = SpeechTarget(text: raw.string, frame: CGRect(x: 200, y: 200, width: 600, height: 80), point: CGPoint(x: 210, y: 210), pid: 1, bundleID: "com.t3tools.t3code", source: "paragraph", attributed: raw)
        let view = SpeechOverlay()
        defer { view.hide() }
        view.show(target: target, state: .playing, rate: 1, fraction: 0.1, error: nil)
        let panel = try XCTUnwrap(app.windows.last { $0.title == "Hey Talk — Read Aloud" })
        XCTAssertFalse(panel.canBecomeKey); XCTAssertFalse(panel.canBecomeMain)
        func findText(_ view: NSView) -> NSTextView? { (view as? NSTextView) ?? view.subviews.compactMap(findText).first }
        let text = try XCTUnwrap(findText(try XCTUnwrap(panel.contentView)))
        let storage = try XCTUnwrap(text.textStorage)
        let bold = try XCTUnwrap(storage.attribute(.font, at: 6, effectiveRange: nil) as? NSFont)
        XCTAssertTrue(NSFontManager.shared.traits(of: bold).contains(.boldFontMask))
        XCTAssertTrue((storage.attribute(.font, at: 11, effectiveRange: nil) as? NSFont)?.fontName.contains("Menlo") == true)
        let original = NSAttributedString(attributedString: storage)
        view.highlight((NSRange(location: 6, length: 4), true))
        XCTAssertEqual(storage.attribute(.foregroundColor, at: 6, effectiveRange: nil) as? NSColor, .clear)
        let lens = try XCTUnwrap(text.subviews.first { $0 is SpeechWordLens } as? SpeechWordLens)
        XCTAssertNil(lens.hitTest(.zero))
        XCTAssertEqual(lens.magnification, 1.035, accuracy: 0.001)
        view.highlight((NSRange(location: 16, length: 4), true))
        XCTAssertEqual(storage.attributedSubstring(from: NSRange(location: 6, length: 4)), original.attributedSubstring(from: NSRange(location: 6, length: 4)))
        view.highlight(nil)
        XCTAssertEqual(NSAttributedString(attributedString: storage), original)
        XCTAssertEqual(text.string, target.text)
    }
    func testPasswordAndInteractiveControlsNeverBecomeSpeechTargets() {
        for role in ["AXButton", "AXTextArea", "AXTextField", "AXComboBox", "AXToolbar"] {
            XCTAssertTrue(SpeechTargetReader.excluded(role: role, subrole: ""))
        }
        XCTAssertTrue(SpeechTargetReader.excluded(role: "AXGroup", subrole: "AXSecureTextField"))
        XCTAssertFalse(SpeechTargetReader.excluded(role: "AXStaticText", subrole: ""))
        XCTAssertTrue(SpeechTargetReader.excluded(role: "AXGroup", subrole: "AXEmptyGroup", valueSettable: true))
        XCTAssertTrue(SpeechTargetReader.excluded(role: "AXStaticText", subrole: "", editableAncestor: true))
    }
}
