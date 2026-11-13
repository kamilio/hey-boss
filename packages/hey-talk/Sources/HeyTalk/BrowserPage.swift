import AppKit

/// Use the focused window's committed document, never editable address-bar text.
enum BrowserPage {
    // Electron apps and terminals also register as HTTPS handlers on macOS.
    static func isBrowser(_ app: NSRunningApplication?) -> Bool {
        guard let id = app?.bundleIdentifier?.lowercased() else { return false }
        return ["com.apple.safari", "com.apple.safaritechnologypreview", "com.google.chrome",
                "org.mozilla.firefox", "org.mozilla.nightly", "com.microsoft.edgemac",
                "com.brave.browser", "com.vivaldi.vivaldi", "company.thebrowser.browser"].contains {
            id == $0 || id.hasPrefix($0 + ".")
        }
    }

    static func url(_ element: AXUIElement, _ attribute: String) -> URL? {
        let value = SpeechAX.value(element, attribute)
        if let url = value as? URL { return url }
        return (value as? String).flatMap(URL.init(string:))
    }

    static func host(_ url: URL?) -> String? {
        guard let url, ["https", "http"].contains(url.scheme?.lowercased()) else { return nil }
        return url.host?.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "."))
    }

    static func documentHost(in window: AXUIElement) -> String? {
        if let document = url(window, "AXDocument") { return host(document) }
        return page(in: window, deadline: ProcessInfo.processInfo.systemUptime + 0.08).flatMap { host($0.url) }
    }

    static func host(in app: NSRunningApplication?) -> String? {
        guard let app, isBrowser(app), AXIsProcessTrusted() else { return nil }
        let ax = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(ax, 0.02)
        AXUIElementSetAttributeValue(ax, "AXManualAccessibility" as CFString, kCFBooleanTrue)
        guard let window = SpeechAX.element(ax, "AXFocusedWindow") else { return nil }
        AXUIElementSetMessagingTimeout(window, 0.02)
        return documentHost(in: window)
    }

    static func page(in window: AXUIElement, deadline: TimeInterval) -> (area: AXUIElement, url: URL)? {
        let document = url(window, "AXDocument")
        var found: (area: AXUIElement, url: URL)?
        var ambiguous = false
        func visit(_ element: AXUIElement, depth: Int) {
            guard !ambiguous, document == nil || found == nil else { return }
            guard depth < 30, ProcessInfo.processInfo.systemUptime < deadline else { ambiguous = true; return }
            AXUIElementSetMessagingTimeout(element, 0.02)
            switch SpeechAX.string(element, "AXRole") {
            case "AXWebArea":
                guard let pageURL = url(element, "AXURL"), document == nil || pageURL == document,
                      let frame = SpeechAX.frame(element), !frame.isEmpty else { return }
                if found != nil { ambiguous = true }
                else { found = (element, pageURL) }
            case "AXWindow", "AXGroup", "AXSplitGroup", "AXTabGroup", "AXScrollArea":
                for child in SpeechAX.value(element, "AXChildren") as? [AXUIElement] ?? [] {
                    visit(child, depth: depth + 1)
                }
            default: break
            }
        }
        visit(window, depth: 0)
        return ambiguous ? nil : found
    }
}
