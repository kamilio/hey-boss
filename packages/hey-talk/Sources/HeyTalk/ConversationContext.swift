import AppKit
import TalkCore

/// Rich reads are only used for an explicitly requested conversation capture.
enum ConversationContext {
    static func focusedWindow(_ app: NSRunningApplication) -> AXUIElement? {
        guard AXIsProcessTrusted() else { return nil }
        let root = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(root, 0.15)
        AXUIElementSetAttributeValue(root, "AXManualAccessibility" as CFString, kCFBooleanTrue)
        return SpeechAX.element(root, "AXFocusedWindow")
    }

    static func text(_ app: NSRunningApplication, window: AXUIElement?, fallback: () -> String?) async throws -> String {
        if let window, SpeechAX.value(window, "AXMinimized") as? Bool == true {
            throw TalkError("The requested window is minimized.")
        }
        var unavailable: String
        do {
            switch app.bundleIdentifier {
            case "md.obsidian":
                unavailable = "Obsidian’s CLI is unavailable. Enable it to read the full note."
                guard let cli = app.bundleURL?.appendingPathComponent("Contents/MacOS/obsidian-cli") else { break }
                // Resolve the file and editor in the same invocation, including unpersisted edits.
                let code = """
                (async()=>{const view=app.workspace.activeLeaf?.view,f=view?.file;
                const header="Vault: "+app.vault.getName()+"\\nFile: "+(f?.path??"(none)");
                if(!f || f.extension!=="md")return header;
                return header+"\\n\\n"+(view.editor?.getValue()??await app.vault.read(f))})()
                """
                let result = try run(cli, ["eval", "code=" + code])
                guard result.hasPrefix("=> Vault: ") else { throw TalkError("Enable Obsidian’s CLI to read the active note.") }
                return String(result.dropFirst(3))
            case "com.tinyspeck.slackmacgap":
                unavailable = window == nil && !AXIsProcessTrusted() ? "Allow Hey Talk in Privacy & Security → Accessibility to identify the Slack conversation." : "Open a Slack DM, channel, or thread to read its history."
                guard let window, let route = slackRoute(app, window: window) else {
                    throw TalkError(unavailable)
                }
                do { return try await SlackTools.history(URL(string: route)!, call: SlackAPI().call) }
                catch { unavailable = error.localizedDescription; throw error }
            case "com.cmuxterm.app":
                unavailable = "cmux’s control socket is unavailable."
                guard let cli = app.bundleURL?.appendingPathComponent("Contents/Resources/bin/cmux") else { break }
                let tree = try run(cli, ["tree"])
                let screen = try? run(cli, ["read-screen", "--lines", "500"])
                return tree + (screen.map { "\n\nFocused terminal (last 500 lines):\n" + $0 } ?? "")
            default: break
            }
        } catch {
            if app.bundleIdentifier == "com.cmuxterm.app", let window, let partial = cmuxWindow(window) {
                return unavailable + "\n" + partial
            }
            if let partial = fallback() { return unavailable + "\nVisible window only:\n" + partial }
            throw TalkError(unavailable)
        }
        if let visible = fallback() { return visible }
        throw TalkError(AXIsProcessTrusted() ? "No accessible window." : "Allow Hey Talk in Privacy & Security → Accessibility.")
    }

    // The default cmux socket policy only admits its own children. The native
    // tab bar, workspace list and rendered terminals remain accessible via AX.
    private static func cmuxWindow(_ window: AXUIElement) -> String? {
        var workspaces = [String](), tabs = [String](), terminals = [String]()
        let deadline = ProcessInfo.processInfo.systemUptime + 0.8
        func visit(_ node: AXUIElement, depth: Int) {
            guard depth < 30, ProcessInfo.processInfo.systemUptime < deadline else { return }
            AXUIElementSetMessagingTimeout(node, 0.03)
            let role = SpeechAX.string(node, "AXRole")
            guard role != "AXWebArea", SpeechAX.value(node, "AXHidden") as? Bool != true else { return }
            if role == "AXCell" {
                let title = SpeechAX.string(node, "AXDescription")
                if !title.isEmpty { workspaces.append(title) }
                return
            }
            if role == "AXTextArea", SpeechAX.string(node, "AXHelp") == "Terminal content area" {
                terminals.append(String(SpeechAX.string(node, "AXValue").suffix(16000)))
                return
            }
            let children = SpeechAX.value(node, "AXChildren") as? [AXUIElement] ?? []
            if children.contains(where: { SpeechAX.string($0, "AXIdentifier").hasPrefix("paneTabBarControl.") || SpeechAX.string($0, "AXRole") == "AXButton" && SpeechAX.value($0, "AXSelected") as? Bool == true }) {
                for tab in children where SpeechAX.string(tab, "AXRole") == "AXButton" && SpeechAX.string(tab, "AXIdentifier").isEmpty {
                    let name = SpeechAX.string(tab, "AXDescription")
                    if !name.isEmpty { tabs.append(name + (SpeechAX.value(tab, "AXSelected") as? Bool == true ? " (selected)" : "")) }
                }
            }
            for child in children { visit(child, depth: depth + 1) }
        }
        visit(window, depth: 0)
        let parts = [("Workspaces", workspaces), ("Visible tabs", tabs), ("Visible terminal text", terminals)]
            .filter { !$0.1.isEmpty }.map { $0.0 + ":\n" + $0.1.joined(separator: "\n") }
        return parts.isEmpty ? nil : String(parts.joined(separator: "\n\n").prefix(65536))
    }

    static func slackURL(_ route: String) -> String? {
        guard let url = URL(string: route), url.scheme == "https", url.host == "app.slack.com",
              let match = url.path.wholeMatch(of: #/\/client\/([TE][A-Z0-9]+)\/([CDG][A-Z0-9]+)(?:\/thread\/([CDG][A-Z0-9]+)-([0-9]{10}\.[0-9]{6}))?\/?/#),
              match.3 == nil || match.3 == match.2 else { return nil }
        return "https://app.slack.com" + url.path
    }

    private static func slackRoute(_ app: NSRunningApplication, window: AXUIElement) -> String? {
        if let field = FocusedInput.read(app: app, forAuto: false), let original = field.window, CFEqual(original, window),
           let doc = field.document, let url = URLComponents(string: doc), url.scheme == "slack",
           let items = url.queryItems, let team = items.first(where: { $0.name == "team" })?.value,
           let channel = items.first(where: { $0.name == "id" })?.value,
           let ts = items.first(where: { $0.name == "thread_ts" })?.value {
            return slackURL("https://app.slack.com/client/" + team + "/" + channel + "/thread/" + channel + "-" + ts)
        }
        let route = SlackTools.route(in: window)
        return route.flatMap { slackURL($0.absoluteString) }
    }

    private static func run(_ cli: URL, _ args: [String]) throws -> String {
        let process = Process(), output = Pipe()
        process.executableURL = cli; process.arguments = args
        process.currentDirectoryURL = URL(fileURLWithPath: "/")
        process.environment = ProcessInfo.processInfo.environment.filter { !["CMUX_WORKSPACE_ID", "CMUX_SURFACE_ID", "CMUX_WINDOW_ID", "CMUX_PANE_ID"].contains($0.key) }
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = output; process.standardError = FileHandle.nullDevice
        try process.run()
        let timeout = DispatchWorkItem { if process.isRunning { kill(process.processIdentifier, SIGKILL) } }
        DispatchQueue.global().asyncAfter(deadline: .now() + 4, execute: timeout)
        defer { timeout.cancel() }
        // Drain the pipe even after the attachment fills, so a large history can't block the child.
        var data = Data(), truncated = false
        while let chunk = try output.fileHandleForReading.read(upToCount: 65536), !chunk.isEmpty {
            let space = 256 * 1024 - data.count
            data.append(chunk.prefix(space)); truncated = truncated || chunk.count > space
        }
        process.waitUntilExit()
        guard process.terminationStatus == 0, !data.isEmpty else { throw TalkError("App context unavailable.") }
        return String(decoding: data, as: UTF8.self) + (truncated ? "\n[Capture truncated at 256 KiB.]" : "")
    }
}
