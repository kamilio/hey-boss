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
        return visibleText(in: window, viewport: viewport, deadline: deadline) { element, name in
            if name == "AXFrame" { return frame(element) }
            return value(element, name)
        }
    }

    static func visibleText<Element>(in root: Element, viewport: CGRect, deadline: TimeInterval,
                                     value: (Element, String) -> Any?) -> String? {
        var text = "", nodes = 0
        func visit(_ element: Element, clip: CGRect, depth: Int) {
            guard depth < 40, nodes < 1500, ProcessInfo.processInfo.systemUptime < deadline else { return }
            nodes += 1
            let role = value(element, "AXRole") as? String ?? ""
            guard value(element, "AXHidden") as? Bool != true,
                  value(element, "AXSubrole") as? String != "AXSecureTextField",
                  !["AXButton", "AXMenuButton", "AXPopUpButton", "AXComboBox", "AXTextField", "AXTextArea", "AXMenuBar"].contains(role),
                  value(element, "AXEditable") as? Bool != true,
                  value(element, "AXEditableAncestor") == nil else { return }
            let frame = value(element, "AXFrame") as? CGRect
            // Chromium can clamp offscreen nodes to a one-pixel strip at the
            // viewport edge while still exposing their complete text values.
            if let frame, frame.width < 2 || frame.height < 8 || !clip.intersects(frame) { return }
            if role == "AXStaticText" {
                // Skip partially clipped blocks too; never send an entire offscreen paragraph.
                guard let frame, !frame.isEmpty, clip.contains(frame),
                      let piece = value(element, "AXValue") as? String, !piece.isEmpty else { return }
                text = String((text + (text.isEmpty ? "" : "\n") + piece.suffix(8000)).suffix(8000))
                return
            }
            let clipped = frame.map { clip.intersection($0) } ?? clip
            let children = (role == "AXScrollArea" ? value(element, "AXVisibleChildren") as? [Element] : nil)
                ?? value(element, "AXChildren") as? [Element] ?? []
            for child in children { visit(child, clip: clipped, depth: depth + 1) }
        }
        visit(root, clip: viewport, depth: 0)
        return text.isEmpty ? nil : text
    }
}
