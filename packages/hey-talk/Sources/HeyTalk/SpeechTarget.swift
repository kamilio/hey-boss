import AppKit
import TalkCore

enum SpeechTargetFailure: LocalizedError {
    case interactive
    var errorDescription: String? { "Hover over a read-only paragraph, outside the editor" }
}

struct SpeechTarget: Equatable {
    let text: String
    let frame: CGRect // Accessibility coordinates: origin at top left of primary screen.
    var point: CGPoint
    let pid: pid_t
    let bundleID: String
    let source: String
    var attributed: NSAttributedString? = nil
    var paragraphs: [SpeechParagraph] = []
    var captureID: UUID? = nil
    var viewport: CGRect? = nil
    func paragraph(containing location: Int) -> SpeechParagraph? {
        paragraphs.first { NSLocationInRange(location, $0.range) }
    }
    var json: [String: Any] {
        ["text": text, "app": bundleID, "source": source,
         "bounds": ["x": frame.minX, "y": frame.minY, "width": frame.width, "height": frame.height]]
    }
}

// Only called on a serial background queue, after the frontmost-app allowlist
// check. Text-marker ranges preserve paragraph boundaries across bold/link spans.
final class SpeechTargetReader {
    private var attached: pid_t?
    private var captureID: UUID?
    private var captured: [(element: AXUIElement, text: String)] = []
    private var capturedOwner: AXUIElement?
    func read(pid: pid_t, bundleID: String, point: CGPoint, preferSelection: Bool) throws -> SpeechTarget {
        guard AXIsProcessTrusted() else { throw TalkError("Allow Hey Talk in Accessibility to read text") }
        let root = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(root, 0.1)
        if attached != pid {
            AXUIElementSetAttributeValue(root, "AXManualAccessibility" as CFString, kCFBooleanTrue)
            attached = pid
        }
        var found: AXUIElement?
        guard AXUIElementCopyElementAtPosition(root, Float(point.x), Float(point.y), &found) == .success,
              let found else { throw TalkError("Hover over a text paragraph") }
        guard !SpeechAX.interactive(found) else { throw SpeechTargetFailure.interactive }
        let hit = nearbyParagraph(in: found, point: point, depth: 0) ?? found
        var hitPID: pid_t = 0
        guard AXUIElementGetPid(hit, &hitPID) == .success, hitPID == pid else { throw TalkError("Hover over an allowed app") }
        var ancestors: [AXUIElement] = []
        var next: AXUIElement? = hit
        for _ in 0..<30 {
            guard let current = next else { break }
            AXUIElementSetMessagingTimeout(current, 0.1)
            ancestors.append(current)
            if SpeechAX.string(current, "AXRole") == "AXWebArea" { break }
            next = SpeechAX.element(current, "AXParent")
        }
        // Never offer speech over buttons, editors, or password fields.
        guard !SpeechPassageStructure.interactiveRegion(ancestors: ancestors, point: point,
            frame: SpeechAX.frame, interactive: SpeechAX.interactive,
            children: { SpeechAX.value($0, "AXChildren") as? [AXUIElement] ?? [] }) else {
            throw SpeechTargetFailure.interactive
        }
        let document = ancestors.last(where: { SpeechAX.string($0,"AXRole") == "AXWebArea" })
        let owner = document ?? hit
        if preferSelection, let range = SpeechAX.value(owner, "AXSelectedTextMarkerRange"),
           let selected = target(owner: owner, range: range, point: point, pid: pid, bundleID: bundleID, source: "selection") {
            return selected
        }
        // A paragraph is often exposed as a group of inline text/style nodes.
        // Chromium advertises position markers but may return nil, and its
        // paragraph expansion can stop at a bold span. Use the nearest bounded
        // text-only block, without collecting a message or window recursively.
        for block in ancestors.prefix(8) {
            let role = SpeechAX.string(block, "AXRole"), subrole = SpeechAX.string(block, "AXSubrole")
            if SpeechPassageStructure.inline(role: role, subrole: subrole) { continue }
            guard ["AXGroup", "AXHeading", "AXParagraph", "AXListItem"].contains(role) else { break }
            let children = SpeechAX.value(block, "AXChildren") as? [AXUIElement] ?? []
            guard !children.isEmpty, children.count <= 256,
                  children.allSatisfy({ SpeechPassageStructure.inline(role: SpeechAX.string($0, "AXRole"), subrole: SpeechAX.string($0, "AXSubrole")) }),
                  let frame = SpeechAX.frame(block), frame.insetBy(dx: -12, dy: -8).contains(point),
                  let range = SpeechAX.parameter(owner, "AXTextMarkerRangeForUIElement", block),
                  let passage = target(owner: owner, range: range, point: point, pid: pid, bundleID: bundleID, source: "paragraph", frame: frame) else { break }
            return capture(passage, block: block, owner: owner)
        }
        var location = point
        if let locationValue = AXValueCreate(.cgPoint, &location),
           let marker = SpeechAX.parameter(owner, "AXTextMarkerForPosition", locationValue),
           let range = SpeechAX.parameter(owner, "AXParagraphTextMarkerRangeForTextMarker", marker),
           let paragraph = target(owner: owner, range: range, point: point, pid: pid, bundleID: bundleID, source: "paragraph"),
           paragraph.frame.insetBy(dx: -2, dy: -2).contains(point) {
            let block = ancestors.prefix(8).first { element in
                guard let range = SpeechAX.parameter(owner, "AXTextMarkerRangeForUIElement", element),
                      let text = SpeechAX.parameter(owner, "AXStringForTextMarkerRange", range) as? String else { return false }
                return text.trimmingCharacters(in: .whitespacesAndNewlines) == paragraph.text
            } ?? hit
            return capture(paragraph, block: block, owner: owner)
        }
        // Native apps may expose a whole paragraph as static text without text
        // markers. No recursive collection of a window or an entire conversation.
        guard SpeechAX.string(hit,"AXRole") == "AXStaticText", let frame = SpeechAX.frame(hit), frame.contains(point) else {
            throw TalkError("This app does not expose a paragraph here")
        }
        let text = SpeechAX.string(hit, "AXValue").trimmingCharacters(in: .whitespacesAndNewlines)
        _ = try SpeechText.chunks(text)
        var passage = SpeechTarget(text: text, frame: frame, point: point, pid: pid, bundleID: bundleID, source: "text")
        var nativeRange = CFRange(location: 0, length: (text as NSString).length)
        if let value = AXValueCreate(.cfRange, &nativeRange) {
            passage.attributed = SpeechAX.parameter(hit, "AXAttributedStringForRange", value) as? NSAttributedString
        }
        return capture(passage, block: hit, owner: owner)
    }
    private func nearbyParagraph(in element: AXUIElement, point: CGPoint, depth: Int) -> AXUIElement? {
        let role = SpeechAX.string(element, "AXRole")
        guard depth < 3, ["AXGroup", "AXParagraph", "AXHeading", "AXListItem", "AXList"].contains(role),
              let frame = SpeechAX.frame(element), frame.insetBy(dx: -12, dy: -8).contains(point) else { return nil }
        let children = SpeechAX.value(element, "AXChildren") as? [AXUIElement] ?? []
        guard !children.isEmpty, children.count <= 64 else { return nil }
        if children.allSatisfy({ SpeechPassageStructure.inline(role: SpeechAX.string($0, "AXRole"), subrole: SpeechAX.string($0, "AXSubrole")) }) { return element }
        // Whitespace hits a container instead of a glyph. Search only its nearby
        // bounded text blocks, never adjacent controls or the whole document.
        for child in children {
            if let block = nearbyParagraph(in: child, point: point, depth: depth + 1) { return block }
        }
        return nil
    }
    private func capture(_ first: SpeechTarget, block: AXUIElement, owner: AXUIElement) -> SpeechTarget {
        var first = first
        first.viewport = visibleViewport(of: block)
        var passages = [first], elements = [block]
        let following = SpeechPassageStructure.following(after: block,
            parent: { SpeechAX.element($0, "AXParent") }, role: { SpeechAX.string($0, "AXRole") },
            subrole: { SpeechAX.string($0, "AXSubrole") }, children: { SpeechAX.value($0, "AXChildren") as? [AXUIElement] ?? [] },
            same: { CFEqual($0, $1) })
        var budget = first.text.utf16.count
        for element in following {
            guard let frame = SpeechAX.frame(element), frame.width > 0, frame.height > 0 else { break }
            let next: SpeechTarget?
            if let range = SpeechAX.parameter(owner, "AXTextMarkerRangeForUIElement", element) {
                next = target(owner: owner, range: range, point: first.point, pid: first.pid, bundleID: first.bundleID, source: "paragraph", frame: frame)
            } else if SpeechAX.string(element, "AXRole") == "AXStaticText" {
                let text = SpeechAX.string(element, "AXValue").trimmingCharacters(in: .whitespacesAndNewlines)
                next = text.isEmpty ? nil : SpeechTarget(text: text, frame: frame, point: first.point, pid: first.pid, bundleID: first.bundleID, source: "text")
            } else { next = nil }
            guard let next, budget + next.text.utf16.count + 1 <= 20_000 else { break }
            budget += next.text.utf16.count + 1; passages.append(next); elements.append(element)
        }
        var result = SpeechPassage.join(passages)
        result.viewport = first.viewport
        let id = UUID(); result.captureID = id; captureID = id
        captured = zip(elements, passages).map { ($0.0, $0.1.text) }; capturedOwner = owner
        return result
    }
    func geometry(for target: SpeechTarget, location: Int) -> (frame: CGRect, viewport: CGRect)? {
        guard target.captureID == captureID, let owner = capturedOwner,
              let index = target.paragraphs.firstIndex(where: { NSLocationInRange(location, $0.range) }), index < captured.count else { return nil }
        let source = captured[index]
        guard let frame = SpeechAX.frame(source.element), frame.width > 0, frame.height > 0 else { return nil }
        // A closed/replaced message must never leave a lens on unrelated text.
        let text: String
        if let range = SpeechAX.parameter(owner, "AXTextMarkerRangeForUIElement", source.element),
           let value = SpeechAX.parameter(owner, "AXStringForTextMarkerRange", range) as? String { text = value }
        else { text = SpeechAX.string(source.element, "AXValue") }
        guard text.trimmingCharacters(in: .whitespacesAndNewlines) == source.text else { return nil }
        guard let viewport = visibleViewport(of: source.element) else { return nil }
        return (frame, viewport)
    }
    private func visibleViewport(of source: AXUIElement) -> CGRect? {
        var viewport = CGRect.infinite, current = SpeechAX.element(source, "AXParent")
        for _ in 0..<30 {
            guard let element = current else { break }
            if let bounds = SpeechAX.frame(element), bounds.width > 0, bounds.height > 0 { viewport = viewport.intersection(bounds) }
            if SpeechAX.string(element, "AXRole") == "AXWindow" { break }
            current = SpeechAX.element(element, "AXParent")
        }
        guard !viewport.isNull, !viewport.isInfinite else { return nil }
        return viewport
    }
    static func excluded(role: String, subrole: String, valueSettable: Bool = false, editableAncestor: Bool = false) -> Bool {
        valueSettable || editableAncestor || subrole == "AXSecureTextField" || ["AXButton", "AXPopUpButton", "AXMenuItem", "AXTextField", "AXTextArea", "AXComboBox", "AXToolbar"].contains(role)
    }
    private func target(owner: AXUIElement, range: CFTypeRef, point: CGPoint, pid: pid_t, bundleID: String, source: String, frame suppliedFrame: CGRect? = nil) -> SpeechTarget? {
        guard let text = SpeechAX.parameter(owner, "AXStringForTextMarkerRange", range) as? String,
              !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, text.utf16.count <= 20_000,
              let frame = suppliedFrame ?? SpeechAX.parameter(owner, "AXBoundsForTextMarkerRange", range).flatMap(SpeechAX.rect),
              frame.width > 0, frame.height > 0 else { return nil }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        var result = SpeechTarget(text: trimmed, frame: frame, point: point, pid: pid, bundleID: bundleID, source: source)
        if let rich = SpeechAX.parameter(owner, "AXAttributedStringForTextMarkerRange", range) as? NSAttributedString {
            let location = (rich.string as NSString).range(of: trimmed)
            if location.location != NSNotFound { result.attributed = rich.attributedSubstring(from: location) }
        }
        return result
    }
}

enum SpeechPassageStructure {
    static func following<Node>(after first: Node, parent: (Node) -> Node?, role: (Node) -> String, subrole: (Node) -> String, children: (Node) -> [Node], same: (Node, Node) -> Bool) -> [Node] {
        var result: [Node] = [], anchor = first, budget = 512
        for _ in 0..<12 {
            guard let container = parent(anchor), structural(role(container)),
                  !SpeechTargetReader.excluded(role: role(container), subrole: subrole(container)) else { break }
            let siblings = children(container)
            guard siblings.count <= 256, let start = siblings.firstIndex(where: { same($0, anchor) }) else { break }
            // A headed body beside action controls is its own content region.
            // Do not append sibling timestamps/captions outside that body.
            if children(anchor).first.map({ role($0) == "AXHeading" }) == true,
               siblings.contains(where: { SpeechTargetReader.excluded(role: role($0), subrole: subrole($0)) }) { break }
            for sibling in siblings.dropFirst(start + 1) {
                guard let next = blocks(in: sibling, role: role, subrole: subrole, children: children, budget: &budget) else { return result }
                result += next
                if result.count >= 63 { return Array(result.prefix(63)) }
            }
            // A list or nested prose wrapper is transparent. A message with
            // controls is a boundary even when those controls precede the text.
            for sibling in siblings.prefix(start) {
                guard blocks(in: sibling, role: role, subrole: subrole, children: children, budget: &budget) != nil else { return result }
            }
            anchor = container
        }
        return result
    }
    private static func structural(_ role: String) -> Bool {
        ["AXGroup", "AXParagraph", "AXHeading", "AXList", "AXListItem"].contains(role)
    }
    static func interactiveRegion<Node>(ancestors: [Node], point: CGPoint, frame: (Node) -> CGRect?, interactive: (Node) -> Bool, children: (Node) -> [Node]) -> Bool {
        if ancestors.contains(where: interactive) { return true }
        // Web editors may place read-only placeholder text over a sibling text
        // area. Reject the overlapping control, without excluding nearby prose.
        var budget = 128
        for ancestor in ancestors.prefix(8) {
            let siblings = children(ancestor)
            guard siblings.count <= budget else { break }
            budget -= siblings.count
            if siblings.contains(where: { frame($0)?.contains(point) == true && interactive($0) }) { return true }
        }
        return false
    }
    static func blocks<Node>(in root: Node, role: (Node) -> String, subrole: (Node) -> String, children: (Node) -> [Node]) -> [Node]? {
        var budget = 256
        return blocks(in: root, role: role, subrole: subrole, children: children, budget: &budget)
    }
    private static func blocks<Node>(in root: Node, role: (Node) -> String, subrole: (Node) -> String, children: (Node) -> [Node], budget: inout Int) -> [Node]? {
        func visit(_ node: Node, depth: Int) -> [Node]? {
            budget -= 1
            guard budget >= 0, depth < 6 else { return nil }
            let kind = role(node)
            if kind == "AXListMarker" { return [] }
            if kind == "AXStaticText" { return [node] }
            guard !SpeechTargetReader.excluded(role: kind, subrole: subrole(node)),
                  structural(kind) else { return nil }
            let items = children(node)
            if items.isEmpty { return kind == "AXGroup" ? [] : nil }
            guard items.count <= 64 else { return nil }
            if items.allSatisfy({ inline(role: role($0), subrole: subrole($0)) }) { return [node] }
            var result: [Node] = []
            for item in items {
                if role(item) == "AXListMarker" { continue }
                guard let nested = visit(item, depth: depth + 1) else { return nil }
                result += nested
                guard result.count <= 64 else { return nil }
            }
            return result
        }
        return visit(root, depth: 0)
    }
    static func inline(role: String, subrole: String) -> Bool {
        role == "AXStaticText" || role == "AXListMarker" || role == "AXLink" || (role == "AXGroup" && [
            "AXStrongStyleGroup", "AXEmphasisStyleGroup", "AXCodeStyleGroup",
            "AXSubscriptStyleGroup", "AXSuperscriptStyleGroup", "AXMarkStyleGroup",
            "AXUnderlineStyleGroup", "AXStrikethroughStyleGroup"
        ].contains(subrole))
    }
}

enum SpeechAX {
    static func interactive(_ element: AXUIElement) -> Bool {
        if SpeechTargetReader.excluded(role: string(element, "AXRole"), subrole: string(element, "AXSubrole")) { return true }
        var writable = DarwinBoolean(false)
        AXUIElementIsAttributeSettable(element, "AXValue" as CFString, &writable)
        return writable.boolValue || self.element(element, "AXEditableAncestor") != nil
    }
    static func value(_ e: AXUIElement, _ name: String) -> CFTypeRef? {
        var value: CFTypeRef?
        return AXUIElementCopyAttributeValue(e, name as CFString, &value) == .success ? value : nil
    }
    static func parameter(_ e: AXUIElement, _ name: String, _ argument: CFTypeRef) -> CFTypeRef? {
        var value: CFTypeRef?
        return AXUIElementCopyParameterizedAttributeValue(e, name as CFString, argument, &value) == .success ? value : nil
    }
    static func element(_ e: AXUIElement, _ name: String) -> AXUIElement? {
        guard let value = value(e, name), CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
        return (value as! AXUIElement)
    }
    static func string(_ e: AXUIElement, _ name: String) -> String { value(e, name) as? String ?? "" }
    static func rect(_ value: CFTypeRef) -> CGRect? {
        guard CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
        var rect = CGRect.zero
        return AXValueGetValue(value as! AXValue, .cgRect, &rect) ? rect : nil
    }
    static func frame(_ e: AXUIElement) -> CGRect? { value(e,"AXFrame").flatMap(rect) }
}
