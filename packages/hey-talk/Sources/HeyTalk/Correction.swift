import Foundation
import TalkCore

struct CorrectionSettings {
    private let defaults: UserDefaults
    init(defaults: UserDefaults = .standard) { self.defaults = defaults }
    var enabled: Bool { defaults.object(forKey: "correctionEnabled") as? Bool ?? true }
    var json: [String: Any] { ["enabled": enabled, "model": "gpt-6-luna"] }
    static func validate(_ args: [String]) throws {
        guard args.isEmpty || (args.count == 1 && ["on", "off", "toggle"].contains(args[0])) else {
            throw TalkError("Use correction [on|off|toggle]")
        }
    }
    func command(_ args: [String]) throws {
        try Self.validate(args)
        guard let action = args.first else { return }
        defaults.set(action == "toggle" ? !enabled : action == "on", forKey: "correctionEnabled")
    }
}

enum CorrectionAction: Equatable {
    case paste(String), rename(String), newThread(project: String?, text: String)

    var text: String {
        switch self {
        case .paste(let text), .rename(let text): return text
        case .newThread(let project, let text):
            if !text.isEmpty { return text }
            return project.map { "New thread in " + $0 } ?? "New thread"
        }
    }
}

// The existing loopback proxy owns OpenAI credentials. Audio never leaves the Mac.
final class Correction {
    private let session: URLSession
    private var task: Task<Void, Never>?

    init(session: URLSession = URLSession(configuration: {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForResource = 30
        configuration.connectionProxyDictionary = [:]
        return configuration
    }())) { self.session = session }

    static func request(text: String, vocabulary: [String], appContext: String? = nil, threadActions: Bool = false) throws -> URLRequest {
        let bundle = Bundle.main.bundleURL.pathExtension == "app" ? Bundle.main : Bundle.module
        func prompt(_ name: String) throws -> String {
            guard let url = bundle.url(forResource: name, withExtension: "md") else {
                throw TalkError("Correction prompt is missing. Reinstall Hey Talk.")
            }
            let text = try String(contentsOf: url, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty else { throw TalkError("Correction prompt is empty.") }
            return text
        }
        func tool(_ name: String, _ arguments: String...) throws -> [String: Any] {
            ["type": "function", "name": name, "description": try prompt(name), "strict": true,
             "parameters": ["type": "object", "properties": Dictionary(uniqueKeysWithValues: arguments.map { ($0, ["type": "string"]) }),
                            "required": arguments, "additionalProperties": false]]
        }
        var tools = [try tool("paste", "text")]
        if threadActions {
            tools.append(try tool("rename_thread", "title"))
            tools.append(try tool("new_thread", "project", "text"))
        }
        var request = URLRequest(url: URL(string: "http://127.0.0.1:8080/v1/responses")!, timeoutInterval: 8)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let access = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".hey-boss/proxy/config.access-keys.json")
        if FileManager.default.fileExists(atPath: access.path) {
            let keys = try JSONSerialization.jsonObject(with: Data(contentsOf: access)) as? [String: Any]
            guard let key = keys?["local"] as? String, !key.isEmpty else { throw TalkError("Hey Proxy’s local access key is invalid.") }
            request.setValue("Bearer " + key, forHTTPHeaderField: "Authorization")
        }
        var values: [String: Any] = ["transcript": text, "vocabulary": vocabulary]
        if let appContext { values["app_context"] = appContext }
        let input = try JSONSerialization.data(withJSONObject: values, options: [.sortedKeys])
        request.httpBody = try JSONSerialization.data(withJSONObject: [
            "model": "gpt-6-luna", "service_tier": "priority", "reasoning": ["effort": "none"], "store": false, "stream": true,
            "instructions": try prompt("correction"), "tools": tools, "tool_choice": "required", "parallel_tool_calls": false, "input": String(decoding: input, as: UTF8.self)
        ])
        return request
    }

    private static func action(_ bytes: URLSession.AsyncBytes, threadActions: Bool) async throws -> CorrectionAction {
        var action: CorrectionAction?
        for try await line in bytes.lines where line.hasPrefix("data:") {
            let data = Data(line.dropFirst(5).utf8)
            guard let event = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
                throw TalkError("Invalid correction event.")
            }
            switch event["type"] as? String {
            case "response.output_item.done":
                guard let item = event["item"] as? [String: Any] else { throw TalkError("Missing correction item.") }
                if item["type"] as? String == "reasoning" { continue }
                guard action == nil, item["type"] as? String == "function_call",
                      let arguments = item["arguments"] as? String,
                      let values = try JSONSerialization.jsonObject(with: Data(arguments.utf8)) as? [String: String] else { throw TalkError("Invalid correction tool call.") }
                switch item["name"] as? String {
                case "paste" where values.count == 1:
                    guard let text = values["text"]?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty
                    else { throw TalkError("Correction returned no text.") }
                    action = .paste(text)
                case "rename_thread" where threadActions && values.count == 1:
                    guard let title = values["title"]?.trimmingCharacters(in: .whitespacesAndNewlines), !title.isEmpty,
                          title.rangeOfCharacter(from: .controlCharacters.union(.newlines)) == nil
                    else { throw TalkError("Invalid thread title.") }
                    action = .rename(title)
                case "new_thread" where threadActions && values.count == 2:
                    guard let project = values["project"]?.trimmingCharacters(in: .whitespacesAndNewlines),
                          project.rangeOfCharacter(from: .controlCharacters) == nil,
                          let text = values["text"]?.trimmingCharacters(in: .whitespacesAndNewlines)
                    else { throw TalkError("Invalid new thread request.") }
                    action = .newThread(project: project.isEmpty ? nil : project, text: text)
                default: throw TalkError("Unavailable correction tool.")
                }
            case "response.completed":
                guard let action else { throw TalkError("Correction returned no tool call.") }
                return action
            case "error", "response.failed", "response.incomplete":
                throw TalkError("Correction did not finish.")
            default: break
            }
        }
        throw TalkError("Correction did not finish.")
    }

    func correct(_ text: String, vocabulary: [String], appContext: String? = nil, threadActions: Bool = false, completion: @escaping (Result<CorrectionAction, Error>) -> Void) {
        task = Task { [session] in
            let result: Result<CorrectionAction, Error>
            do {
                let request = try Self.request(text: text, vocabulary: vocabulary, appContext: appContext, threadActions: threadActions)
                let (bytes, response) = try await session.bytes(for: request)
                guard let response = response as? HTTPURLResponse else { throw TalkError("Correction is unavailable.") }
                guard (200..<300).contains(response.statusCode) else {
                    throw TalkError("Correction proxy returned HTTP \(response.statusCode).")
                }
                result = .success(try await Self.action(bytes, threadActions: threadActions))
            } catch { result = .failure(error) }
            await MainActor.run { completion(result) }
        }
    }
    func cancel() { task?.cancel(); task = nil }
    deinit { task?.cancel() }
}
