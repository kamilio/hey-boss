import AppKit

enum AppContext {
    // Called once per dictation on a background queue, alongside transcription.
    static func capture(pid: pid_t) -> String? {
        guard AXIsProcessTrusted() else { return nil }
        let deadline = ProcessInfo.processInfo.systemUptime + 0.2
        func value(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
            guard ProcessInfo.processInfo.systemUptime < deadline else { return nil }
            AXUIElementSetMessagingTimeout(element, 0.02)
            var result: CFTypeRef?
            return AXUIElementCopyAttributeValue(element, name as CFString, &result) == .success ? result : nil
        }
        func frame(_ element: AXUIElement) -> CGRect? {
            guard let position = value(element, "AXPosition"), CFGetTypeID(position) == AXValueGetTypeID(),
                  let size = value(element, "AXSize"), CFGetTypeID(size) == AXValueGetTypeID() else { return nil }
            var point = CGPoint.zero, dimensions = CGSize.zero
            guard AXValueGetValue(position as! AXValue, .cgPoint, &point),
                  AXValueGetValue(size as! AXValue, .cgSize, &dimensions) else { return nil }
            return CGRect(origin: point, size: dimensions)
        }
        let app = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(app, 0.02)
        AXUIElementSetAttributeValue(app, "AXManualAccessibility" as CFString, kCFBooleanTrue)
        guard let raw = value(app, "AXFocusedWindow"), CFGetTypeID(raw) == AXUIElementGetTypeID() else { return nil }
        let window = raw as! AXUIElement
        guard value(window, "AXMinimized") as? Bool != true, let viewport = frame(window), !viewport.isEmpty else { return nil }
        let bundle = NSRunningApplication(processIdentifier: pid)?.bundleIdentifier
        let isCmux = bundle == "com.cmuxterm.app"
        let conversationOnly = bundle == "com.t3tools.t3code" || bundle == "com.tinyspeck.slackmacgap"
        return visibleText(in: window, viewport: viewport, deadline: deadline, conversationOnly: conversationOnly) { element, name in
            if name == "AXFrame" { return frame(element) }
            // cmux/Ghostty exposes the rendered viewport as AXValue, not an editor draft.
            if name == "AXVisibleText" { return isCmux ? value(element, "AXValue") as? String : nil }
            return value(element, name)
        }
    }

    static func visibleText<Element>(in root: Element, viewport: CGRect, deadline: TimeInterval, conversationOnly: Bool = false,
                                     value: (Element, String) -> Any?) -> String? {
        var sections: [(title: String, text: String)] = [], nodes = 0
        func append(_ text: String, path: [String]) {
            if conversationOnly && !path.contains(where: { ["Conversation", "Current input"].contains($0) }) { return }
            let title = path.isEmpty ? "Visible text" : path.joined(separator: " / ")
            if sections.last?.title == title { sections[sections.count - 1].text += "\n" + text.suffix(8000) }
            else { sections.append((title, String(text.suffix(8000)))) }
            sections[sections.count - 1].text = String(sections[sections.count - 1].text.suffix(8000))
        }
        func visit(_ element: Element, clip: CGRect, depth: Int, path: [String]) -> String? {
            guard depth < 40, nodes < 1500, ProcessInfo.processInfo.systemUptime < deadline else { return nil }
            nodes += 1
            let role = value(element, "AXRole") as? String ?? ""
            if role == "AXHeading" {
                // Speaker headings may be hidden, screen-reader-only labels.
                // They label visible sibling text; their contents aren't captured.
                let children = value(element, "AXChildren") as? [Element] ?? []
                return children.compactMap { value($0, "AXValue") as? String }.joined().trimmingCharacters(in: .whitespacesAndNewlines)
            }
            let label = ["AXGroup", "AXScrollArea", "AXSplitGroup", "AXTabGroup", "AXList", "AXTextField", "AXTextArea"].contains(role)
                ? ["AXTitle", "AXDescription"].compactMap { value(element, $0) as? String }
                    .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.first { !$0.isEmpty } : nil
            if conversationOnly, let label, ["Threads", "Thread breadcrumb", "Thread details", "Notifications"].contains(label) { return nil }
            if label == "Reactions" || label == "Message actions" { return nil }
            let terminal = role == "AXTextArea" ? value(element, "AXVisibleText") as? String : nil
            let input = ["AXTextField", "AXTextArea"].contains(role) &&
                (value(element, "AXFocused") as? Bool == true || label?.lowercased().hasPrefix("message") == true)
                ? value(element, "AXValue") as? String : nil
            guard value(element, "AXHidden") as? Bool != true,
                  value(element, "AXSubrole") as? String != "AXSecureTextField",
                  !["AXButton", "AXMenuButton", "AXPopUpButton", "AXComboBox", "AXMenuBar", "AXOutline", "AXToolbar"].contains(role),
                  terminal != nil || input != nil || (!["AXTextArea", "AXTextField"].contains(role) && value(element, "AXEditable") as? Bool != true &&
                                      value(element, "AXEditableAncestor") == nil) else { return nil }
            let frame = value(element, "AXFrame") as? CGRect
            // Chromium can clamp offscreen nodes to a one-pixel strip at the
            // viewport edge while still exposing their complete text values.
            let layoutFrame = frame.flatMap { $0.width >= 2 && $0.height >= 8 ? $0 : nil }
            if let layoutFrame, !clip.intersects(layoutFrame) { return nil }
            if role == "AXStaticText" || terminal != nil || input != nil {
                // Skip partially clipped blocks too; never send an entire offscreen paragraph.
                guard let frame = layoutFrame, input != nil ? viewport.intersects(frame) : clip.contains(frame),
                      let piece = terminal ?? input ?? value(element, "AXValue") as? String, !piece.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
                append(piece, path: terminal != nil ? ["Terminal"] : input != nil ? ["Current input"] : path)
                return nil
            }
            // display:contents and virtual-list wrappers may have no rectangle.
            // Their children's own rectangles still determine visibility.
            let clipped = layoutFrame.map { clip.intersection($0) } ?? clip
            let children = (role == "AXScrollArea" ? value(element, "AXVisibleChildren") as? [Element] : nil)
                ?? value(element, "AXChildren") as? [Element] ?? []
            var region = path
            if ["AXGroup", "AXScrollArea", "AXSplitGroup", "AXTabGroup", "AXList"].contains(role), let label,
               !label.isEmpty, label.count <= 80, region.last != label { region.append(label) }
            if role == "AXList", let label, !label.isEmpty { region = ["Conversation", String(label.prefix(80))] }
            var current = region
            for child in children {
                if let heading = visit(child, clip: clipped, depth: depth + 1, path: current), !heading.isEmpty {
                    current = conversationOnly && ["You", "T3 Code"].contains(heading)
                        ? ["Conversation", heading] : region + [String(heading.prefix(80))]
                }
            }
            return nil
        }
        _ = visit(root, clip: viewport, depth: 0, path: [])
        if sections.contains(where: { $0.title == "Terminal" }) { sections.removeAll { $0.title != "Terminal" } }
        var remaining = 8000, result: [String] = []
        for section in sections.reversed() {
            let header = "## \(section.title)\n"
            let available = remaining - header.count - (result.isEmpty ? 0 : 2)
            guard available > 0 else { break }
            let text = String(section.text.suffix(available))
            result.append(header + text)
            remaining -= header.count + text.count + (result.count == 1 ? 0 : 2)
        }
        return result.isEmpty ? nil : result.reversed().joined(separator: "\n\n")
    }
}
