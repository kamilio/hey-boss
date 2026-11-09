import Foundation
import TalkCore

extension SlackTools {
    /// Route has already been checked against the focused Slack window.
    static func history(_ route: URL, call: (String, [String: String]) async throws -> [String: Any]) async throws -> String {
        let path = route.pathComponents, identity = try await call("auth.test", [:])
        guard [identity["team_id"] as? String, identity["enterprise_id"] as? String].contains(path[2]) else { throw TalkError("Different Slack workspace.") }
        var lines = [String: String](), parents = [String](), remaining = 256 * 1024
        let deadline = ProcessInfo.processInfo.systemUptime + 75
        func pages(thread: String?) async throws {
            var params = ["channel": path[3], "limit": "200"], seen = Set<String>()
            params["ts"] = thread
            repeat {
                guard ProcessInfo.processInfo.systemUptime < deadline else { throw TalkError("Time limit reached.") }
                let page = try await call(thread == nil ? "conversations.history" : "conversations.replies", params)
                guard let messages = page["messages"] as? [[String: Any]] else { throw TalkError("Slack returned no history page.") }
                for message in messages {
                    guard let ts = message["ts"] as? String, lines[ts] == nil else { continue }
                    let user = (message["user_profile"] as? [String: Any])?["display_name"] as? String ?? (message["username"] ?? message["user"] ?? message["bot_id"]) as? String ?? "Slack"
                    var body = message["text"] as? String ?? ""
                    for file in message["files"] as? [[String: Any]] ?? [] {
                        if let name = file["title"] as? String ?? file["name"] as? String { body += "\nFile: " + name }
                    }
                    if body.isEmpty, let blocks = message["blocks"], let data = try? JSONSerialization.data(withJSONObject: blocks) {
                        body = String(decoding: data, as: UTF8.self)
                    }
                    let reply = (message["thread_ts"] as? String).flatMap { $0 == ts ? nil : " (thread " + $0 + ")" } ?? ""
                    let line = "[" + ts + "] " + user + reply + ": " + body
                    lines[ts] = String(decoding: line.utf8.prefix(remaining), as: UTF8.self)
                    remaining -= min(remaining, line.utf8.count + 1)
                    guard remaining > 0 else { throw TalkError("256 KiB limit reached.") }
                    if thread == nil, (message["reply_count"] as? Int ?? 0) > 0 { parents.append(ts) }
                }
                let cursor = (page["response_metadata"] as? [String: Any])?["next_cursor"] as? String ?? ""
                if cursor.isEmpty {
                    guard page["has_more"] as? Bool != true else { throw TalkError("Slack did not return the next page.") }
                    break
                }
                guard seen.insert(cursor).inserted else { throw TalkError("Slack repeated a page.") }
                params["cursor"] = cursor
            } while true
        }
        var note = ""
        do {
            let thread = path.count > 5 ? path[5].split(separator: "-").last.map(String.init) : nil
            try await pages(thread: thread)
            for ts in parents { try await pages(thread: ts) }
        } catch {
            guard !lines.isEmpty else { throw error }
            note = "\n[Partial Slack history: " + error.localizedDescription + "]"
        }
        return route.absoluteString + "\n" + (lines.isEmpty ? "(No messages)" : lines.keys.sorted().compactMap { lines[$0] }.joined(separator: "\n")) + note
    }
}
