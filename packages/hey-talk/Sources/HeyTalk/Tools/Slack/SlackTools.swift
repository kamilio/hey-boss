import AppKit
import TalkCore

enum SlackAction: Equatable {
    case users(String), chat([String]), channel(String)

    var description: String {
        switch self {
        case .users: return "List Slack users"
        case .chat(let people): return "Open Slack chat with " + people.joined(separator: ", ")
        case .channel(let name): return "Open Slack channel #" + name
        }
    }
}

enum SlackTools {
    static func definitions(prompt: (String) throws -> String) throws -> [[String: Any]] {
        let people: [String: Any] = ["type": "array", "items": ["type": "string"]]
        return try [
            ToolPackage.tool("slack_list_users", ["query": ["type": "string"]], prompt: prompt),
            ToolPackage.tool("slack_open_chat", ["people": people], prompt: prompt),
            ToolPackage.tool("slack_open_channel", ["name": ["type": "string"]], prompt: prompt)
        ]
    }

    static func action(_ name: String, values: [String: Any]) throws -> SlackAction {
        if name == "slack_list_users", values.count == 1, let query = values["query"] as? String { return .users(query) }
        if name == "slack_open_channel", values.count == 1, let value = values["name"] as? String {
            let channel = value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "#"))
            guard !channel.isEmpty else { throw TalkError("Missing channel name.") }
            return .channel(channel)
        }
        if name == "slack_open_chat", values.count == 1, let names = values["people"] as? [String], (1...8).contains(names.count) {
            let people = names.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            guard !people.contains("") else { throw TalkError("Missing Slack recipient.") }
            return .chat(people)
        }
        throw TalkError("Invalid Slack tool call.")
    }

    private static func names(_ user: [String: Any]) -> [String] {
        let profile = user["profile"] as? [String: Any]
        return [user["name"], user["username"], user["real_name"], profile?["real_name"], profile?["display_name"], profile?["email"]].compactMap { $0 as? String }
    }

    static func recipients(_ people: [String], users: [[String: Any]]) throws -> [String] {
        try people.map { person in
            let query = String(person.drop(while: { $0 == "@" })).lowercased()
            let exact = users.filter { names($0).map { $0.lowercased() }.contains(query) }
            let hits = exact.isEmpty ? users.filter { names($0).contains { $0.lowercased().split(separator: " ").contains(Substring(query)) } } : exact
            guard hits.count == 1, let id = hits[0]["id"] as? String else {
                throw TalkError(hits.isEmpty ? "Person not found" : "Use a full name")
            }
            return id
        }
    }

    enum Result { case chat(URL), users([String]) }

    static func route(_ app: NSRunningApplication?) -> URL? {
        guard let app, app.bundleIdentifier == "com.tinyspeck.slackmacgap" else { return nil }
        let ax = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(ax, 0.1)
        AXUIElementSetAttributeValue(ax, "AXManualAccessibility" as CFString, kCFBooleanTrue)
        guard let window = SpeechAX.element(ax, "AXFocusedWindow") else { return nil }
        var pending = [window]
        for _ in 0..<80 {
            guard let node = pending.popLast() else { break }
            if SpeechAX.string(node, "AXRole") == "AXWebArea" {
                let raw = SpeechAX.value(node, "AXURL")
                if let url = raw as? URL ?? (raw as? String).flatMap(URL.init(string:)),
                   url.scheme == "https", url.host == "app.slack.com", url.pathComponents.count >= 3,
                   url.pathComponents[1] == "client", url.pathComponents[2].wholeMatch(of: #/^[TE][A-Z0-9]+$/#) != nil { return url }
                continue
            }
            pending += SpeechAX.value(node, "AXChildren") as? [AXUIElement] ?? []
        }
        return nil
    }

    @MainActor static func perform(_ action: SlackAction, team: String, focused: () -> Bool,
                                   call: (String, [String: String]) async throws -> [String: Any]) async throws -> Result {
        guard focused() else { throw TalkError("Focus Slack") }
        guard try await call("auth.test", [:])["team_id"] as? String == team else { throw TalkError("Different Slack workspace") }
        func directory(_ query: String) async throws -> [[String: Any]] {
            var users = [[String: Any]](), cursor = "", seen = Set<String>(), page = 1
            repeat {
                guard focused() else { throw TalkError("Slack focus changed") }
                if !query.isEmpty {
                    let result = try await call("search.modules.people", ["module":"people", "query":query, "page":String(page), "count":"100", "team_id":team])
                    guard let items = result["items"] as? [[String: Any]] else { throw TalkError("Directory unavailable") }
                    users += items
                    if page >= ((result["pagination"] as? [String: Any])?["page_count"] as? Int ?? 1) { break }
                    page += 1
                } else {
                    guard seen.insert(cursor).inserted else { throw TalkError("Directory unavailable") }
                    let result = try await call("users.list", ["limit":"999", "team_id":team, "cursor":cursor])
                    guard let members = result["members"] as? [[String: Any]] else { throw TalkError("Directory unavailable") }
                    users += members
                    cursor = (result["response_metadata"] as? [String: Any])?["next_cursor"] as? String ?? ""
                    if cursor.isEmpty { break }
                }
            } while true
            return users.filter { $0["deleted"] as? Bool != true && $0["is_bot"] as? Bool != true && $0["id"] as? String != "USLACKBOT" }
        }
        if case .users(let query) = action {
            let users = try await directory(query)
            return .users(users.compactMap { user in
                guard let handle = (user["name"] ?? user["username"]) as? String else { return nil }
                return (user["real_name"] as? String ?? (user["profile"] as? [String: Any])?["real_name"] as? String ?? handle) + "  ·  @" + handle
            }.sorted { $0.localizedCaseInsensitiveCompare($1) == .orderedAscending })
        }
        let channel: String
        switch action {
        case .chat(let people):
            var ids = [String]()
            for person in people {
                ids += try recipients([person], users: try await directory(String(person.drop(while: { $0 == "@" }))))
            }
            guard focused() else { throw TalkError("Slack focus changed") }
            let opened = try await call("conversations.open", ["users": ids.joined(separator: ","), "prevent_creation":"true"])
            guard let id = (opened["channel"] as? [String: Any])?["id"] as? String else { throw TalkError("Chat not found") }
            channel = id
        case .channel(let name):
            let response = try await call("search.modules.channels", ["module": "channels", "query": name, "count": "100", "page": "1", "team_id": team])
            let matches = (response["items"] as? [[String: Any]] ?? []).filter {
                ($0["name"] as? String)?.lowercased() == name && $0["is_archived"] as? Bool != true &&
                    ($0["team"] as? String ?? team) == team
            }
            guard matches.count == 1, let id = matches[0]["id"] as? String else { throw TalkError("No unique channel") }
            channel = id
        case .users: preconditionFailure()
        }
        guard focused(), channel.wholeMatch(of: #/^[CDG][A-Z0-9]+$/#) != nil,
              let link = URL(string: "slack://channel?team=\(team)&id=\(channel)") else { throw TalkError("Slack chat unavailable") }
        return .chat(link)
    }

    @MainActor private static var directoryPanel: NSPanel?
    @MainActor static func showUsers(_ users: [String]) {
        directoryPanel?.close()
        let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 320, height: 280),
                            styleMask: [.titled, .closable, .resizable, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        panel.title = "Slack people · " + String(users.count)
        panel.level = .floating
        let scroll = NSTextView.scrollableTextView()
        let view = scroll.documentView as! NSTextView
        view.isEditable = false; view.font = .systemFont(ofSize: 13); view.textContainerInset = NSSize(width: 12, height: 10)
        view.string = users.isEmpty ? "No matching people." : users.joined(separator: "\n")
        panel.contentView = scroll; panel.center(); panel.orderFrontRegardless()
        directoryPanel = panel
    }

    @MainActor static func navigate(_ action: SlackAction, app: NSRunningApplication, team: String,
                                    isCurrent: () -> Bool) async throws {
        func focused() -> Bool { isCurrent() && app.isActive && route(app)?.pathComponents[2] == team }
        let result = try await perform(action, team: team, focused: focused, call: SlackAPI().call)
        guard focused() else { throw TalkError("Slack focus changed") }
        switch result {
        case .users(let users): showUsers(users)
        case .chat(let url):
            guard let bundle = app.bundleURL, let id = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first(where: { $0.name == "id" })?.value else { throw TalkError("Slack unavailable") }
            directoryPanel?.close()
            _ = try await NSWorkspace.shared.open([url], withApplicationAt: bundle, configuration: .init())
            for _ in 0..<35 {
                guard isCurrent() else { throw CancellationError() }
                if let path = route(app)?.pathComponents, path.count >= 4, path[2] == team, path[3] == id { return }
                try await Task.sleep(nanoseconds: 100_000_000)
            }
            throw TalkError("Chat did not open")
        }
    }
}
