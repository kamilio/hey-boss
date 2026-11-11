import AppKit
import TalkCore

enum CatchUpSource: Equatable {
    case t3(environment: String, thread: String), slack(URL)

    static func current() throws -> Self {
        guard let app = FocusedInput.keyboardApp, let window = ConversationContext.focusedWindow(app) else {
            throw TalkError("Focus a T3 or Slack conversation and allow Hey Talk Accessibility access.")
        }
        switch app.bundleIdentifier {
        case "com.t3tools.t3code":
            let document = FocusedInput.read(app: app, forAuto: false)?.document ??
                BrowserPage.page(in: window, deadline: ProcessInfo.processInfo.systemUptime + 0.4)?.url.absoluteString
            if let document, let link = FocusedInput.threadLink(document) { return .t3(environment: link.pathComponents[1], thread: link.lastPathComponent) }
        case "com.tinyspeck.slackmacgap":
            if let path = ConversationContext.slackRoute(app, window: window), let url = URL(string: path) { return .slack(url) }
        default: break
        }
        throw TalkError("Open the T3 thread or Slack conversation you want to hear.")
    }

    func passage() async throws -> SpokenRecap {
        switch self {
        case let .t3(environment, thread): return try await T3Server.shared.catchUp(thread, environment: environment)
        case let .slack(route): return try await SlackTools.catchUp(route, call: SlackAPI().call)
        }
    }
}

struct SpokenRecap: Encodable {
    let lastMessage: String
    let replies: String

    func summarize() async throws -> String {
        let bundle = Bundle.main.bundleURL.pathExtension == "app" ? Bundle.main : Bundle.module
        guard let url = bundle.url(forResource: "catch_up", withExtension: "md") else { throw TalkError("Reinstall Hey Talk to restore the recap prompt.") }
        var request = try ProxyResponses.request()
        request.timeoutInterval = 45
        request.httpBody = try JSONSerialization.data(withJSONObject: [
            "model":"gpt-6-luna", "store":false, "reasoning":["effort":"none"], "max_output_tokens":800,
            "instructions":try String(contentsOf: url, encoding: .utf8), "input":String(decoding: try JSONEncoder().encode(self), as: UTF8.self)])
        let (data, response) = try await URLSession.shared.data(for: request)
        guard (response as? HTTPURLResponse)?.statusCode == 200,
              let body = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              body["status"] as? String == "completed", let items = body["output"] as? [[String: Any]] else {
            throw TalkError("Couldn’t prepare a recap. Try Catch Up again.")
        }
        let text = items.filter { $0["type"] as? String == "message" }.flatMap { $0["content"] as? [[String: Any]] ?? [] }
            .filter { $0["type"] as? String == "output_text" }.compactMap { $0["text"] as? String }
            .joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { throw TalkError("No spoken recap was returned.") }
        return text
    }
}
