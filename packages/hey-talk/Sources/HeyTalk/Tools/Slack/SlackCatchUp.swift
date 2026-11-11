import Foundation
import TalkCore

extension SlackTools {
    static func catchUp(_ route: URL, call: (String, [String: String]) async throws -> [String: Any]) async throws -> SpokenRecap {
        guard ConversationContext.slackURL(route.absoluteString) != nil else { throw TalkError("Open a Slack conversation.") }
        let path = route.pathComponents, identity = try await call("auth.test", [:])
        guard [identity["team_id"] as? String, identity["enterprise_id"] as? String].contains(path[2]),
              let me = identity["user_id"] as? String else { throw TalkError("Different Slack workspace.") }
        let thread = path.count > 5 ? path[5].split(separator: "-").last.map(String.init) : nil
        var lastMessage: String?, replies = [[String: Any]]()
        try await messagePages(channel: path[3], thread: thread, deadline: ProcessInfo.processInfo.systemUptime + 60, call: call) { page in
            // Channel pages go backwards; thread pages go forwards.
            for message in page {
                if message["user"] as? String == me, message["subtype"] == nil || message["subtype"] as? String == "file_share" {
                    lastMessage = message["text"] as? String ?? ""
                    if thread == nil { return false }
                    replies.removeAll()
                } else if thread == nil || lastMessage != nil { replies.append(message) }
            }
            return true
        }
        guard let lastMessage else { throw TalkError("No message from you in this conversation yet.") }
        var names = [String: String](), lines = [String]()
        if thread == nil { replies.reverse() }
        for reply in replies {
            try Task.checkCancellation()
            let parts = [reply["text"] as? String] + (reply["files"] as? [[String: Any]] ?? []).map { $0["title"] as? String }
            let text = parts.compactMap { $0 }.joined(separator: " ").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty else { continue }
            var name = (reply["user_profile"] as? [String: Any])?["display_name"] as? String ?? reply["username"] as? String
            if (name ?? "").isEmpty, let id = reply["user"] as? String {
                if names[id] == nil {
                    let user = try await call("users.info", ["user":id])["user"] as? [String: Any]
                    let profile = user?["profile"] as? [String: Any]
                    names[id] = [profile?["display_name"],user?["real_name"],user?["name"]].compactMap { $0 as? String }.first { !$0.isEmpty } ?? id
                }
                name = names[id]
            }
            lines.append((name ?? "Slack") + ": " + text)
        }
        guard !lines.isEmpty else { throw TalkError("No replies since your last message.") }
        return SpokenRecap(lastMessage: lastMessage, replies: lines.joined(separator: "\n"))
    }
}
