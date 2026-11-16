import AppKit

/// Read only the selected range; never touch the clipboard.
final class SelectionCapture: NSObject, NSMenuItemValidation {
    private var monitor: Any?
    private var down: CGPoint?
    private var session = UUID()
    private let queue = DispatchQueue(label: "hey-talk.selection", qos: .userInitiated)
    private var terminalHintShown = false
    enum Mode: String {
        case dictation = "dictationSelectionCapture", conversation = "conversationSelectionCapture"
    }
    private let defaults: UserDefaults
    private var mode = Mode.dictation
    private var chat: String?

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        super.init()
        defaults.register(defaults: [Mode.conversation.rawValue: true])
    }

    func setConversation(_ state: [String: Any]?) {
        let nextMode: Mode = state == nil ? .dictation : .conversation
        let nextChat = state?["selection_chat"] as? String
        if nextMode != mode || nextChat != chat {
            session = UUID(); down = nil
        }
        mode = nextMode; chat = nextChat
        updateMonitor()
    }

    func menuItem(_ mode: Mode? = nil) -> NSMenuItem {
        let item = NSMenuItem(title: "", action: #selector(toggle(_:)), keyEquivalent: "")
        item.target = self; item.representedObject = mode?.rawValue
        _ = validateMenuItem(item)
        return item
    }

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        let enabled = defaults.bool(forKey: item.representedObject as? String ?? mode.rawValue)
        item.title = enabled ? "Selection Capture: On" : "Selection Capture: Off"
        item.state = enabled ? .on : .off
        return true
    }

    @objc private func toggle(_ sender: NSMenuItem) {
        let key = sender.representedObject as? String ?? mode.rawValue
        let enable = !defaults.bool(forKey: key)
        if enable && !AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary) {
            toast("Selection Capture is off", "Allow Hey Talk in Accessibility, then turn capture on.")
            return
        }
        defaults.set(enable, forKey: key)
        updateMonitor()
        _ = validateMenuItem(sender)
        if enable {
            toast("Selection Capture is on", "Highlight text in your browser, cmux, T3, or Slack.")
        }
    }

    var shouldMonitor: Bool { defaults.bool(forKey: mode.rawValue) && (mode == .dictation || chat != nil) }

    private func updateMonitor() {
        guard shouldMonitor else { if monitor != nil { stop() }; return }
        guard monitor == nil, AXIsProcessTrusted() else { return }
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
        let addressOnly: Bool
        if keyboard {
            let modifiers = event.modifierFlags.intersection([.command, .option, .control, .shift])
            addressOnly = modifiers == .command && ["l", "a"].contains(event.charactersIgnoringModifiers?.lowercased())
            guard addressOnly || (modifiers.contains(.shift) && (123...126).contains(event.keyCode)) else { return }
        } else {
            defer { down = nil }
            guard let down, let point else { return }
            addressOnly = hypot(point.x - down.x, point.y - down.y) < 3 && event.clickCount < 2 && !event.modifierFlags.contains(.shift)
        }
        capture(at: keyboard ? nil : point, addressOnly: addressOnly, insert: false)
    }

    func insertSelection() {
        guard AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary) else {
            toast("Selection not added", "Allow Hey Talk in Accessibility to capture selected text."); return
        }
        capture(at: nil, addressOnly: false, insert: true)
    }

    private func capture(at point: CGPoint?, addressOnly: Bool, insert: Bool) {
        guard let app = NSWorkspace.shared.frontmostApplication else { return }
        let browser = BrowserPage.isBrowser(app)
        guard browser || (!addressOnly && ["com.cmuxterm.app", "com.t3tools.t3code", "com.tinyspeck.slackmacgap"].contains(app.bundleIdentifier)) else {
            if insert { toast("Selection not added", "Select text in your browser, cmux, T3, or Slack.") }
            return
        }
        let id = session, time = Date()
        queue.async {
            guard app.isActive else { return }
            let capture = Self.read(app, at: point, addressOnly: addressOnly)
            DispatchQueue.main.async {
                guard self.session == id, insert || self.monitor != nil else { return }
                guard let capture else {
                    if insert { self.toast("Selection not added", "No selected text in the focused window.") }
                    return
                }
                if capture.text.isEmpty {
                    if insert || !self.terminalHintShown {
                        self.terminalHintShown = true
                        self.toast("cmux", "Hold Shift while dragging to select terminal text.")
                    }
                    return
                }
                let stamp = time.formatted(date: .omitted, time: .standard)
                var request = Self.notice(app: app.localizedName ?? "Selection", detail: ([stamp] + capture.source).joined(separator: "  ·  "), text: capture.text)
                request["selection_chat"] = self.chat
                request["selection_time"] = time.timeIntervalSince1970
                if insert { request["command"] = "selection_insert" }
                self.send(request)
            }
        }
    }

    private static func read(_ app: NSRunningApplication, at point: CGPoint?, addressOnly: Bool) -> (text: String, source: [String])? {
        guard let window = ConversationContext.focusedWindow(app) else { return nil }
        let root = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(root, 0.08)
        var node: AXUIElement?
        if let point {
            guard AXUIElementCopyElementAtPosition(root, Float(point.x), Float(point.y), &node) == .success else { return nil }
        } else { node = SpeechAX.element(root, "AXFocusedUIElement") }
        guard let first = node, let owner = SpeechAX.element(first, "AXWindow"), CFEqual(owner, window) else { return nil }
        // For a click or ⌘L/⌘A, never reuse a selection elsewhere on the page.
        if addressOnly && SpeechAX.string(first, "AXRole") != "AXTextField" { return nil }
        var chain = [AXUIElement]()
        for _ in 0..<48 {
            guard let item = node else { break }
            AXUIElementSetMessagingTimeout(item, 0.045)
            guard SpeechAX.string(item, "AXSubrole") != "AXSecureTextField" else { return nil }
            chain.append(item)
            if SpeechAX.string(item, "AXRole") == "AXWebArea" || CFEqual(item, window) { break }
            node = SpeechAX.element(item, "AXParent")
        }
        if BrowserPage.isBrowser(app), let last = chain.last, CFEqual(last, window),
           let field = chain.first(where: { SpeechAX.string($0, "AXRole") == "AXTextField" }),
           isAddressField(identifier: SpeechAX.string(field, "AXIdentifier"), domID: SpeechAX.string(field, "AXDOMIdentifier"), description: SpeechAX.string(field, "AXDescription")) {
            let text = SpeechAX.string(field, "AXSelectedText")
            guard !text.isEmpty, !text.allSatisfy(\.isWhitespace) else { return nil }
            let page = BrowserPage.url(window, "AXDocument") ?? BrowserPage.page(in: window, deadline: ProcessInfo.processInfo.systemUptime + 0.08)?.url
            return (text, [SpeechAX.string(window, "AXTitle"), "Address bar", page?.absoluteString ?? ""].filter { !$0.isEmpty })
        }
        guard !addressOnly else { return nil }
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

    static func isAddressField(identifier: String, domID: String, description: String) -> Bool {
        identifier == "WEB_BROWSER_ADDRESS_AND_SEARCH_FIELD" || domID == "urlbar-input" ||
        description == "Address and search bar"
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
        queue.async {
            let reply = try? TalkService.requestDesktop(request)
            if request["command"] as? String == "selection_insert", reply?["status"] as? String != "ok" {
                self.toast("Selection not added", "Couldn’t attach the selection to the current Hey Boss chat.")
            }
        }
    }
}
