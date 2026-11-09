import AppKit
import ScreenCaptureKit
import TalkCore

enum WindowInspection {
    static func capture(target: String, mode: String) async throws -> [String: Any] {
        guard ["accessibility", "screenshot", "both"].contains(mode) else { throw TalkError("Unknown capture format.") }
        let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
        let visible = windows.filter { $0[kCGWindowLayer as String] as? Int == 0 }
        var apps = [NSRunningApplication]()
        for item in visible {
            guard let pid = item[kCGWindowOwnerPID as String] as? pid_t, !apps.contains(where: { $0.processIdentifier == pid }),
                  let app = NSRunningApplication(processIdentifier: pid), app.activationPolicy == .regular else { continue }
            apps.append(app)
        }
        let query = target.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let app: NSRunningApplication?
        switch query {
        case "active", "": app = apps.first { !["local.hey-talk", "local.hey-boss.desktop"].contains($0.bundleIdentifier) }
        case "browser": app = apps.first(where: BrowserPage.isBrowser)
        default: app = (apps + NSWorkspace.shared.runningApplications).first { $0.localizedName?.lowercased() == query || $0.bundleIdentifier?.lowercased() == query }
        }
        let gmail = query == "gmail" && app == nil
        var selectedWindow: AXUIElement?
        var owner = app
        if gmail {
            guard AXIsProcessTrusted() else { throw TalkError("Allow Hey Talk in Privacy & Security → Accessibility to find Gmail.") }
            var browserWindows = [(NSRunningApplication, AXUIElement)]()
            for browser in apps.filter(BrowserPage.isBrowser) {
                let root = AXUIElementCreateApplication(browser.processIdentifier)
                AXUIElementSetMessagingTimeout(root, 0.15)
                AXUIElementSetAttributeValue(root, "AXManualAccessibility" as CFString, kCFBooleanTrue)
                for window in SpeechAX.value(root, "AXWindows") as? [AXUIElement] ?? [] {
                    guard SpeechAX.value(window, "AXMinimized") as? Bool != true else { continue }
                    if BrowserPage.documentHost(in: window) == "mail.google.com" {
                        selectedWindow = window; owner = browser; break
                    }
                    browserWindows.append((browser, window))
                }
                if owner != nil { break }
            }
            if owner == nil {
                for (browser, window) in browserWindows {
                    guard let tab = gmailTab(in: window), AXUIElementPerformAction(tab, kAXPressAction as CFString) == .success else { continue }
                    for _ in 0..<10 {
                        try await Task.sleep(nanoseconds: 80_000_000)
                        if BrowserPage.documentHost(in: window) == "mail.google.com" { owner = browser; selectedWindow = window; break }
                    }
                    break
                }
            }
        }
        guard let owner else { throw TalkError(gmail ? "Select your Gmail tab in a browser window, then try again." : "No visible window for " + target + ".") }
        let name = owner.localizedName ?? target
        var result: [String: Any] = ["ok": true, "source": gmail ? "Gmail · " + name : name]
        if mode != "screenshot" { selectedWindow = selectedWindow ?? ConversationContext.focusedWindow(owner) }
        // Start the image with the text read, before paging a long chat history.
        let window = selectedWindow
        async let image = mode == "accessibility" ? nil : try await screenshot(owner, window: window, visible: visible)
        var textError: Error?
        if mode != "screenshot" {
            do {
                result["text"] = try await ConversationContext.text(owner, window: selectedWindow) { selectedWindow.map(snapshot) }
            } catch { textError = error }
        }
        if mode != "accessibility" {
            do {
                result["jpeg"] = try await image
                if let textError { result["text"] = "Text unavailable: " + textError.localizedDescription }
            } catch {
                guard let text = result["text"] as? String else {
                    if let textError { throw TalkError(textError.localizedDescription + " " + error.localizedDescription) }
                    throw error
                }
                result["text"] = text + "\n\nScreenshot unavailable: " + error.localizedDescription
            }
        } else if let textError { throw textError }
        if gmail, let selectedWindow, BrowserPage.documentHost(in: selectedWindow) != "mail.google.com" { throw TalkError("The Gmail tab changed during capture. Try again.") }
        return result
    }

    private static func screenshot(_ owner: NSRunningApplication, window selectedWindow: AXUIElement?, visible: [[String: Any]]) async throws -> String {
        guard CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() else { throw TalkError("Allow Hey Talk in Privacy & Security → Screen & System Audio Recording, then retry.") }
        let content = try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: true)
        let candidates = content.windows.filter { $0.owningApplication?.processID == owner.processIdentifier && $0.windowLayer == 0 }
        let chosen: SCWindow?
        if let ax = selectedWindow {
            let title = SpeechAX.string(ax, "AXTitle"), bounds = bounds(ax)
            let frames = candidates.filter { $0.frame == bounds }
            let matches = frames.count == 1 ? frames : frames.filter { !title.isEmpty && $0.title == title }
            chosen = matches.count == 1 ? matches[0] : nil
        } else {
            let front = visible.first { $0[kCGWindowOwnerPID as String] as? pid_t == owner.processIdentifier }
            chosen = candidates.first { $0.windowID == front?[kCGWindowNumber as String] as? CGWindowID }
        }
        guard let window = chosen, window.frame.width > 0, window.frame.height > 0 else { throw TalkError("Could not identify the requested window in " + (owner.localizedName ?? "the app") + ".") }
        let config = SCStreamConfiguration()
        let scale = min(2, 1920 / max(window.frame.width, window.frame.height))
        config.width = max(1, Int(window.frame.width * scale)); config.height = max(1, Int(window.frame.height * scale))
        config.showsCursor = false; config.ignoreShadowsSingleWindow = true
        let image = try await SCScreenshotManager.captureImage(contentFilter: SCContentFilter(desktopIndependentWindow: window), configuration: config)
        guard let data = NSBitmapImageRep(cgImage: image).representation(using: .jpeg, properties: [.compressionFactor: 0.86]) else { throw TalkError("Could not encode screenshot.") }
        return data.base64EncodedString()
    }

    // Only inspect browser chrome for tabs. A page can give its own controls any label.
    private static func gmailTab(in window: AXUIElement) -> AXUIElement? {
        let deadline = ProcessInfo.processInfo.systemUptime + 0.3
        var queue = [window], index = 0
        while index < queue.count, index < 300, ProcessInfo.processInfo.systemUptime < deadline {
            let node = queue[index]; index += 1
            AXUIElementSetMessagingTimeout(node, 0.03)
            let role = SpeechAX.string(node, "AXRole")
            if role == "AXWebArea" { continue }
            if role == "AXRadioButton" || role == "AXTab" {
                if ["AXTitle", "AXDescription"].contains(where: {
                    let title = SpeechAX.string(node, $0)
                    return title == "Gmail" || title.hasSuffix(" - Gmail")
                }) { return node }
            }
            queue += (SpeechAX.value(node, "AXChildren") as? [AXUIElement] ?? []).prefix(300 - min(300, queue.count))
        }
        return nil
    }

    private static func bounds(_ node: AXUIElement) -> CGRect? {
        if let rect = SpeechAX.frame(node) { return rect }
        guard let position = SpeechAX.value(node, "AXPosition"), CFGetTypeID(position) == AXValueGetTypeID(),
              let size = SpeechAX.value(node, "AXSize"), CFGetTypeID(size) == AXValueGetTypeID() else { return nil }
        var point = CGPoint.zero, dimensions = CGSize.zero
        guard AXValueGetValue(position as! AXValue, .cgPoint, &point), AXValueGetValue(size as! AXValue, .cgSize, &dimensions) else { return nil }
        return CGRect(origin: point, size: dimensions)
    }

    private static func snapshot(_ window: AXUIElement) -> String {
        let deadline = ProcessInfo.processInfo.systemUptime + 1.5
        var lines = [String](), remaining = 12000, nodes = 0
        func visit(_ node: AXUIElement, depth: Int, inside viewport: CGRect?) {
            guard remaining > 0, depth < 40, nodes < 1200, ProcessInfo.processInfo.systemUptime < deadline else { return }
            nodes += 1; AXUIElementSetMessagingTimeout(node, 0.04)
            guard SpeechAX.value(node, "AXHidden") as? Bool != true, SpeechAX.string(node, "AXSubrole") != "AXSecureTextField" else { return }
            let frame = bounds(node)
            if let frame, !frame.isEmpty, let viewport, !viewport.intersects(frame) { return }
            let role = SpeechAX.string(node, "AXRole")
            var fields = [String]()
            for key in ["AXTitle", "AXDescription", "AXValue"] {
                let field = SpeechAX.string(node, key).trimmingCharacters(in: .whitespacesAndNewlines)
                if !field.isEmpty, !fields.contains(field) { fields.append(field) }
            }
            if !fields.isEmpty {
                let line = String((String(repeating: " ", count: min(depth, 8)) + role + ": " + fields.joined(separator: " · ")).prefix(remaining))
                lines.append(line); remaining -= line.count + 1
            }
            let clip = role == "AXScrollArea" ? frame.flatMap { viewport?.intersection($0) ?? $0 } ?? viewport : viewport
            let children = SpeechAX.value(node, "AXVisibleChildren") as? [AXUIElement] ?? SpeechAX.value(node, "AXChildren") as? [AXUIElement] ?? []
            for child in children {
                guard remaining > 0, nodes < 1200, ProcessInfo.processInfo.systemUptime < deadline else { break }
                visit(child, depth: depth + 1, inside: clip)
            }
        }
        visit(window, depth: 0, inside: bounds(window))
        return lines.isEmpty ? "This window exposes no accessible text or controls." : lines.joined(separator: "\n")
    }
}
