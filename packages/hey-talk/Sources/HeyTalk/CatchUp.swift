import AppKit
import NaturalLanguage
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

    @MainActor func summarize(onSentence: (String) throws -> Void) async throws {
        let bundle = Bundle.main.bundleURL.pathExtension == "app" ? Bundle.main : Bundle.module
        guard let url = bundle.url(forResource: "catch_up", withExtension: "md") else { throw TalkError("Reinstall Hey Talk to restore the recap prompt.") }
        var request = try ProxyResponses.request()
        request.timeoutInterval = 45
        request.httpBody = try JSONSerialization.data(withJSONObject: [
            "model":"gpt-6-luna", "store":false, "stream":true, "reasoning":["effort":"none"],
            "instructions":try String(contentsOf: url, encoding: .utf8), "input":String(decoding: try JSONEncoder().encode(self), as: UTF8.self)])
        let (bytes, response) = try await URLSession.shared.bytes(for: request)
        guard let http = response as? HTTPURLResponse else { throw TalkError("The recap service is unavailable.") }
        guard (200..<300).contains(http.statusCode) else { throw TalkError("The recap service returned HTTP \(http.statusCode).") }
        var stream = RecapStream()
        for try await line in bytes.lines where line.hasPrefix("data:") {
            try Task.checkCancellation()
            let data = line.dropFirst(5).trimmingCharacters(in: .whitespaces)
            if data == "[DONE]" { break }
            if try stream.receive(data, onSentence: onSentence) { return }
        }
        throw TalkError("The recap connection closed before the response finished.")
    }
}

struct RecapStream {
    private let sentences = NLTokenizer(unit: .sentence)
    private var pending = ""
    private var emitted = false

    mutating func receive(_ data: String, onSentence: (String) throws -> Void) throws -> Bool {
        guard let event = try JSONSerialization.jsonObject(with: Data(data.utf8)) as? [String: Any] else {
            throw TalkError("Invalid recap stream.")
        }
        switch event["type"] as? String {
        case "response.output_text.delta":
            pending += event["delta"] as? String ?? ""
            // Keep the last fragment until the tokenizer sees the next sentence:
            // a delta ending in a period might be half of a decimal or abbreviation.
            while !pending.isEmpty {
                sentences.string = pending
                let range = sentences.tokenRange(at: pending.startIndex)
                guard !range.isEmpty, pending[range.upperBound...].contains(where: { !$0.isWhitespace }) else { break }
                let sentence = pending[range].trimmingCharacters(in: .whitespacesAndNewlines)
                pending = String(pending[range.upperBound...].drop(while: \.isWhitespace))
                if !sentence.isEmpty { try onSentence(sentence); emitted = true }
            }
        case "response.completed":
            let rest = pending.trimmingCharacters(in: .whitespacesAndNewlines)
            if !rest.isEmpty { try onSentence(rest); emitted = true }
            guard emitted else { throw TalkError("The recap service returned no spoken text.") }
            return true
        case "error", "response.failed":
            throw TalkError("The recap service failed while generating the response.")
        case "response.incomplete":
            let response = event["response"] as? [String: Any]
            let details = response?["incomplete_details"] as? [String: Any]
            if details?["reason"] as? String == "max_output_tokens" {
                throw TalkError("The recap reached the service’s output limit.")
            }
            throw TalkError("The recap service stopped before it finished the response.")
        default: break
        }
        return false
    }
}
