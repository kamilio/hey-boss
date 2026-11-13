import AppKit

/// A session-only probe. No clipboard, history, or conversation delivery.
final class SelectionCapture: NSObject, NSMenuItemValidation {
    private var monitor: Any?
    private var down: CGPoint?
    private var session = UUID()
    private let queue = DispatchQueue(label: "hey-talk.selection", qos: .userInitiated)
    private var panel: NSPanel?
    private var dismiss: Timer?
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
        if enabled { stop() }
        else if AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary) {
            monitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .leftMouseUp, .keyUp]) { [weak self] in self?.event($0) }
        }
        _ = validateMenuItem(sender)
        if !enabled && !AXIsProcessTrusted() {
            show("Selection Capture is off", detail: "Allow Hey Talk in Accessibility, then turn capture on.", quote: nil)
        } else if enabled {
            show("Selection Capture is on", detail: "Highlight text in your browser, cmux, T3, or Slack.", quote: nil)
        }
    }

    func stop() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil; session = UUID(); down = nil
        dismiss?.invalidate(); panel?.orderOut(nil); panel = nil
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
            guard app.isActive, let capture = Self.read(app, at: hit) else { return }
            DispatchQueue.main.async {
                guard self.session == id, self.enabled else { return }
                let stamp = time.formatted(date: .omitted, time: .standard)
                self.show(app.localizedName ?? "Selection", detail: ([stamp] + capture.source).joined(separator: "  ·  "), quote: capture.text)
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
        guard let text, !text.allSatisfy(\.isWhitespace) else { return nil }
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

    private func show(_ title: String, detail: String, quote: String?) {
        dismiss?.invalidate(); panel?.orderOut(nil)
        let card = NSPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        card.level = .floating; card.isFloatingPanel = true; card.hidesOnDeactivate = false
        card.isOpaque = false; card.backgroundColor = .clear; card.hasShadow = true
        card.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        let glass = NSVisualEffectView(); glass.material = .popover; glass.state = .active
        glass.wantsLayer = true; glass.layer?.cornerRadius = 15; glass.layer?.masksToBounds = true
        let width: CGFloat = 386, inset: CGFloat = 16, inner = width - 2 * inset
        func label(_ s: String, font: NSFont, color: NSColor, lines: Int) -> NSTextField {
            let l = NSTextField(wrappingLabelWithString: s)
            l.font = font; l.textColor = color; l.maximumNumberOfLines = lines; l.cell?.truncatesLastVisibleLine = true
            l.toolTip = s; l.preferredMaxLayoutWidth = inner
            return l
        }
        let heading = label(title, font: .systemFont(ofSize: 13, weight: .semibold), color: TalkDesign.active, lines: 1)
        let meta = label(detail, font: .systemFont(ofSize: 11), color: .secondaryLabelColor, lines: 3)
        let metaH = ceil(meta.fittingSize.height)
        var y = inset
        if let quote {
            let scroller = NSScrollView()
            let body = NSTextView()
            body.drawsBackground = false; body.isEditable = false; body.isSelectable = true
            body.font = .systemFont(ofSize: 13); body.textContainerInset = .zero; body.textContainer?.lineFragmentPadding = 0
            body.string = quote; body.frame.size.width = inner
            body.textContainer?.containerSize = NSSize(width: inner, height: .greatestFiniteMagnitude)
            body.layoutManager?.ensureLayout(for: body.textContainer!)
            let h = ceil(body.layoutManager!.usedRect(for: body.textContainer!).height)
            scroller.frame = NSRect(x: inset, y: y, width: inner, height: min(108, h))
            scroller.drawsBackground = false; scroller.hasVerticalScroller = h > 108; scroller.autohidesScrollers = true
            scroller.documentView = body; body.frame.size = NSSize(width: scroller.contentSize.width, height: h)
            glass.addSubview(scroller); y += scroller.frame.height + 9
        }
        meta.frame = NSRect(x: inset, y: y, width: inner, height: metaH); glass.addSubview(meta); y += metaH + 4
        heading.frame = NSRect(x: inset, y: y, width: inner, height: 18); glass.addSubview(heading)
        let size = NSSize(width: width, height: y + 18 + inset)
        guard let screen = NSScreen.screens.first(where: { $0.frame.contains(NSEvent.mouseLocation) }) ?? NSScreen.main else { return }
        let visible = screen.visibleFrame
        card.contentView = glass; card.setFrame(NSRect(x: visible.maxX - width - 14, y: visible.maxY - size.height - 12, width: width, height: size.height), display: true)
        card.title = "Selection Capture"; card.orderFrontRegardless(); panel = card
        dismiss = Timer.scheduledTimer(withTimeInterval: 6, repeats: true) { [weak self, weak card] timer in
            guard let card, !card.frame.contains(NSEvent.mouseLocation) else { return }
            timer.invalidate(); card.orderOut(nil); self?.panel = nil
        }
    }
}
