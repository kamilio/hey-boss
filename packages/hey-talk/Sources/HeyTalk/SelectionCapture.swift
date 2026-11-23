import AppKit

/// Hyper+C reads only the selected range; never touches the clipboard.
final class SelectionCapture {
    struct Selection {
        let time: Date
        let source, text: String

        static func attaching(_ selections: [Selection], to action: CorrectionAction) -> CorrectionAction {
            guard !selections.isEmpty else { return action }
            func transcript(_ text: String) -> String {
                let markers = selections.indices.map { "[\($0 + 1)]" }.joined(separator: " ")
                let excerpts = selections.enumerated().map { "[\($0.offset + 1)] " + $0.element.source + "\n" + $0.element.text }
                return text + " " + markers + "\n\n" + excerpts.joined(separator: "\n\n")
            }
            switch action {
            case .paste(let text): return .paste(transcript(text))
            case .newThread(let project, let text): return .newThread(project: project, text: transcript(text))
            default: return action
            }
        }
    }

    private let queue = DispatchQueue(label: "hey-talk.selection", qos: .userInitiated)

    func capture() -> Task<Selection?, Never> {
        let time = Date()
        guard AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary),
              let app = FocusedInput.keyboardApp else {
            toast("Selection not added", "Allow Hey Talk in Accessibility to capture selected text.")
            return Task { nil }
        }
        return Task { @MainActor in
            let result: (text: String, source: [String])? = await withCheckedContinuation { continuation in
                queue.async {
                    guard FocusedInput.keyboardApp?.processIdentifier == app.processIdentifier else {
                        continuation.resume(returning: nil); return
                    }
                    continuation.resume(returning: Self.read(app))
                }
            }
            guard let result else {
                toast("Selection not added", "No selected text in the focused window."); return nil
            }
            guard !result.text.isEmpty else {
                toast("cmux", "Hold Shift while dragging to select terminal text."); return nil
            }
            let detail = ([time.formatted(date: .omitted, time: .standard)] + result.source).joined(separator: "  ·  ")
            return Selection(time: time, source: (app.localizedName ?? "Selection") + " · " + detail, text: result.text)
        }
    }

    func show(_ selection: Selection, insert: Bool, chat: String? = nil) {
        var request = Self.notice(app: "Selection captured", detail: selection.source, text: selection.text)
        request["selection_source"] = selection.source
        request["selection_time"] = selection.time.timeIntervalSince1970
        request["selection_chat"] = chat
        if insert { request["command"] = "selection_insert" }
        send(request)
    }

    private static func read(_ app: NSRunningApplication) -> (text: String, source: [String])? {
        guard let window = ConversationContext.focusedWindow(app) else { return nil }
        let root = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(root, 0.08)
        AXUIElementSetAttributeValue(root, "AXManualAccessibility" as CFString, kCFBooleanTrue)
        var node = SpeechAX.element(root, "AXFocusedUIElement")
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
        if BrowserPage.isBrowser(app), let last = chain.last, CFEqual(last, window),
           let field = chain.first(where: { SpeechAX.string($0, "AXRole") == "AXTextField" }),
           isAddressField(identifier: SpeechAX.string(field, "AXIdentifier"), domID: SpeechAX.string(field, "AXDOMIdentifier"), description: SpeechAX.string(field, "AXDescription")) {
            let text = SpeechAX.string(field, "AXSelectedText")
            guard !text.isEmpty, !text.allSatisfy(\.isWhitespace) else { return nil }
            let page = BrowserPage.url(window, "AXDocument") ?? BrowserPage.page(in: window, deadline: ProcessInfo.processInfo.systemUptime + 0.08)?.url
            return (text, [SpeechAX.string(window, "AXTitle"), "Address bar", page?.absoluteString ?? ""].filter { !$0.isEmpty })
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

    func toast(_ title: String, _ message: String) {
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
