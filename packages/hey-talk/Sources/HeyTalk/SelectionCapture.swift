import AppKit

/// Read only the selected range; never touch the clipboard.
final class SelectionCapture: NSObject, NSMenuItemValidation {
    private var monitor: Any?
    private var down: CGPoint?
    private var session = UUID()
    private let queue = DispatchQueue(label: "hey-talk.selection", qos: .userInitiated)
    private var terminalHintShown = false
    private var manual = false
    private var mutedChat: String?
    var conversation: String? {
        didSet {
            guard oldValue != conversation else { return }
            mutedChat = nil
            session = UUID()
            updateMonitor()
        }
    }
    var enabled: Bool { monitor != nil }

    func menuItem() -> NSMenuItem {
        let item = NSMenuItem(title: enabled ? "Selection Capture: On" : "Selection Capture: Off", action: #selector(toggle(_:)), keyEquivalent: "")
        item.target = self
        item.state = enabled ? .on : .off
        return item
    }

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        item.title = enabled ? "Selection Capture: On" : "Selection Capture: Off"
        item.state = enabled ? .on : .off
        return true
    }

    @objc private func toggle(_ sender: NSMenuItem) {
        if enabled { manual = false; mutedChat = conversation }
        else if AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary) {
            manual = conversation == nil; mutedChat = nil
        }
        updateMonitor()
        _ = validateMenuItem(sender)
        if !enabled && !AXIsProcessTrusted() {
            toast("Selection Capture is off", "Allow Hey Talk in Accessibility, then turn capture on.")
        } else if enabled {
            toast("Selection Capture is on", "Highlight text in your browser, cmux, T3, or Slack.")
        }
    }

    private func updateMonitor() {
        guard manual || (conversation != nil && mutedChat != conversation) else { if enabled { stop() }; return }
        guard !enabled, AXIsProcessTrusted() else { return }
        monitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .leftMouseUp, .keyUp]) { [weak self] in self?.event($0) }
    }

    func stop() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil; session = UUID(); down = nil
        terminalHintShown = false
        send(["command": "selection_notice", "sync": false])
    }

    private func event(_ event: NSEvent) {
        let point = event.cgEvent?.location
        if event.type == .leftMouseDown { down = point; return }
        let keyboard = event.type == .keyUp
        if keyboard {
            guard event.modifierFlags.contains(.shift), (123...126).contains(event.keyCode) else { return }
        } else {
            defer { down = nil }
            guard let down, let point, hypot(point.x - down.x, point.y - down.y) >= 3 ||
                    event.clickCount >= 2 || event.modifierFlags.contains(.shift) else { return }
        }
        guard let app = NSWorkspace.shared.frontmostApplication,
              BrowserPage.isBrowser(app) || ["com.cmuxterm.app", "com.t3tools.t3code", "com.tinyspeck.slackmacgap"].contains(app.bundleIdentifier) else { return }
        let id = session, time = Date(), hit = keyboard ? nil : point
        queue.async {
            guard app.isActive else { return }
            let capture = Self.read(app, at: hit)
            DispatchQueue.main.async {
                guard self.session == id, self.enabled else { return }
                guard let capture else { return }
                if capture.text.isEmpty {
                    if !self.terminalHintShown {
                        self.terminalHintShown = true
                        self.toast("cmux", "Hold Shift while dragging to select terminal text.")
                    }
                    return
                }
                let stamp = time.formatted(date: .omitted, time: .standard)
                var request = Self.notice(app: app.localizedName ?? "Selection", detail: ([stamp] + capture.source).joined(separator: "  ·  "), text: capture.text)
                request["selection_chat"] = self.conversation
                request["selection_time"] = time.timeIntervalSince1970
                self.send(request)
            }
        }
    }

    private static func read(_ app: NSRunningApplication, at point: CGPoint?) -> (text: String, source: [String])? {
        guard let window = ConversationContext.focusedWindow(app) else { return nil }
        let root = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(root, 0.08)
        var node: AXUIElement?
        if let point {
            guard AXUIElementCopyElementAtPosition(root, Float(point.x), Float(point.y), &node) == .success else { return nil }
        } else { node = SpeechAX.element(root, "AXFocusedUIElement") }
        guard let first = node, let owner = SpeechAX.element(first, "AXWindow"), CFEqual(owner, window) else { return nil }
        var chain = [AXUIElement]()
        for _ in 0..<48 {
            guard let item = node else { break }
            AXUIElementSetMessagingTimeout(item, 0.045)
            guard SpeechAX.string(item, "AXSubrole") != "AXSecureTextField" else { return nil }
            chain.append(item)
            if SpeechAX.string(item, "AXRole") == "AXWebArea" || CFEqual(item, window) { break }
            node = SpeechAX.element(item, "AXParent")
        }
        var text: String?
        for item in chain {
            if let s = SpeechAX.value(item, "AXSelectedText") as? String, !s.isEmpty { text = s; break }
            if let range = SpeechAX.value(item, "AXSelectedTextMarkerRange"),
               let s = SpeechAX.parameter(item, "AXStringForTextMarkerRange", range) as? String, !s.isEmpty { text = s; break }
        }
        guard let text, !text.allSatisfy(\.isWhitespace) else {
            // A mouse-aware terminal program can own the drag; do not guess its quote from AXValue.
            return app.bundleIdentifier == "com.cmuxterm.app" && chain.contains(where: { SpeechAX.string($0, "AXHelp") == "Terminal content area" }) ? ("", []) : nil
        }
        var source = [SpeechAX.string(window, "AXTitle")]
        if let area = chain.last, SpeechAX.string(area, "AXRole") == "AXWebArea" {
            let raw = SpeechAX.value(area, "AXURL")
            let url = (raw as? URL)?.absoluteString ?? raw as? String
            if let url {
                if app.bundleIdentifier == "com.t3tools.t3code" {
                    let field = FocusedInput(app: app, element: area, window: window, document: url, composerLabel: nil)
                    if let project = field.projectButton?.name { source.append(project) }
                    source.append(FocusedInput.threadLink(url)?.absoluteString ?? url)
                } else { source.append(url) }
            }
        } else if app.bundleIdentifier == "com.cmuxterm.app",
                  let terminal = chain.first(where: { SpeechAX.string($0, "AXHelp") == "Terminal content area" }) {
            var field = FocusedInput(app: app, element: terminal, window: window, document: nil, composerLabel: nil)
            field.rememberCmuxTab()
            if let tab = field.cmuxTab { source.append(SpeechAX.string(tab, "AXDescription")) }
        }
        return (text, source.filter { !$0.isEmpty })
    }

    static func notice(app: String, detail: String, text: String) -> [String: Any] {
        let content = detail + "\n\n" + text
        let longest = content.split(whereSeparator: { $0 != "`" }).map(\.count).max() ?? 0
        let fence = String(repeating: "`", count: max(3, longest + 1))
        return ["command": "selection_notice", "sync": false, "title": app,
                "selection_text": text, "selection_source": app + " · " + detail,
                "description": String(text.prefix(240)), "question": fence + "\n" + content + "\n" + fence]
    }

    private func toast(_ title: String, _ message: String) {
        send(["command": "toast", "title": title, "question": message, "icon": "text.cursor", "sync": false])
    }

    private func send(_ request: [String: Any]) {
        queue.async { _ = try? TalkService.requestDesktop(request) }
    }
}
