import Foundation
import TalkCore

struct CorrectionSettings {
    private let defaults: UserDefaults
    init(defaults: UserDefaults = .standard) { self.defaults = defaults }
    var enabled: Bool { defaults.object(forKey: "correctionEnabled") as? Bool ?? true }
    var bedrockProfile: String? { defaults.string(forKey: "correctionBedrockProfile") }
    var model: String { bedrockProfile ?? "gpt-6-luna" }
    var json: [String: Any] { ["enabled": enabled, "model": model] }
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
    case slack(SlackAction)
    case paste(String), rename(String), clearInput, newThread(project: String?, text: String)
    case listThreads, nextThread, switchThread(Int), settleThread, pinThread(Bool)

    var text: String {
        switch self {
        case .slack(let action): return action.description
        case .paste(let text), .rename(let text): return text
        case .clearInput: return "Clear input"
        case .nextThread: return "Go to the next T3 chat"
        case .listThreads: return "List T3 threads"
        case .switchThread(let position): return "Go to T3 chat \(position)"
        case .settleThread: return "Settle T3 thread"
        case .pinThread(let pinned): return pinned ? "Pin T3 thread" : "Unpin T3 thread"
        case .newThread(let project, let text):
            if !text.isEmpty { return text }
            return project.map { "New thread in " + $0 } ?? "New thread"
        }
    }
}

struct CorrectionTimeout: LocalizedError {
    let retry: () -> Void
    var errorDescription: String? { "AI correction timed out." }
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

    static func request(text: String, vocabulary: [String], appContext: String? = nil, app: [String: String]? = nil, package: ToolPackage? = nil, canClear: Bool = false, recent: [[String]] = [], history: [[String: Any]] = [], settings: CorrectionSettings = CorrectionSettings()) throws -> URLRequest {
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
        if canClear { tools.append(try tool("clear_input")) }
        if let package { tools += try package.definitions(prompt: prompt) }
        let profile = settings.bedrockProfile
        let path = profile == nil ? "/v1/responses" : "/bedrock/v1/responses"
        var request = URLRequest(url: URL(string: "http://127.0.0.1:8080" + path)!)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let access = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".hey-boss/proxy/config.access-keys.json")
        if FileManager.default.fileExists(atPath: access.path) {
            let keys = try JSONSerialization.jsonObject(with: Data(contentsOf: access)) as? [String: Any]
            guard let key = keys?["local"] as? String, !key.isEmpty else { throw TalkError("Hey Proxy’s local access key is invalid.") }
            request.setValue("Bearer " + key, forHTTPHeaderField: "Authorization")
        }
        var values: [String: Any] = ["transcript": text, "vocabulary": vocabulary]
        values["recent_commands"] = recent
        if let appContext { values["app_context"] = appContext }
        if let app { values["app"] = app }
        let input = try JSONSerialization.data(withJSONObject: values, options: [.sortedKeys])
        let message = String(decoding: input, as: UTF8.self)
        let conversation: Any = history.isEmpty ? message as Any : [["role": "user", "content": message]] + history
        var body: [String: Any] = [
            "model": settings.model, "reasoning": ["effort": "none"], "store": false, "stream": true,
            "instructions": try prompt("correction"), "tools": tools, "tool_choice": history.isEmpty ? "required" : "auto", "parallel_tool_calls": false,
            "include": ["reasoning.encrypted_content"], "input": conversation
        ]
        if profile == nil { body["service_tier"] = "priority" }
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        return request
    }

    private static func action(_ bytes: URLSession.AsyncBytes, package: ToolPackage?, canClear: Bool) async throws -> (CorrectionAction?, [[String: Any]]) {
        var items: [[String: Any]] = []
        var action: CorrectionAction?
        for try await line in bytes.lines where line.hasPrefix("data:") {
            let data = Data(line.dropFirst(5).utf8)
            guard let event = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
                throw TalkError("Invalid correction event.")
            }
            switch event["type"] as? String {
            case "response.output_item.done":
                guard let item = event["item"] as? [String: Any] else { throw TalkError("Missing correction item.") }
                if item["type"] as? String == "message" { continue }
                items.append(item)
                if item["type"] as? String == "reasoning" { continue }
                guard action == nil, item["type"] as? String == "function_call",
                      let arguments = item["arguments"] as? String,
                      let values = try JSONSerialization.jsonObject(with: Data(arguments.utf8)) as? [String: Any] else { throw TalkError("Invalid correction tool call.") }
                switch item["name"] as? String {
                case "clear_input" where canClear && values.isEmpty: action = .clearInput
                case "paste" where values.count == 1:
                    guard let text = (values["text"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty
                    else { throw TalkError("Correction returned no text.") }
                    action = .paste(text)
                case let name?:
                    guard let package else { throw TalkError("Unavailable correction tool.") }
                    action = try package.action(name, values: values)
                default: throw TalkError("Unavailable correction tool.")
                }
            case "response.completed":
                return (action, items)
            case "error", "response.failed", "response.incomplete":
                throw TalkError("Correction did not finish.")
            default: break
            }
        }
        throw TalkError("Correction did not finish.")
    }

    func correct(_ text: String, vocabulary: [String], appContext: String? = nil, app: (() -> [String: String]?)? = nil, package: ToolPackage? = nil, canClear: Bool = false, recent: [[String]] = [],
                 perform: ((CorrectionAction, @escaping (String) -> Void) -> Void)? = nil,
                 completion: @escaping (Result<CorrectionAction, Error>) -> Void) {
        func turn(_ history: [[String: Any]], last: CorrectionAction?, count: Int) {
            task = Task { @MainActor in
                do {
                    let request = try Self.request(text: text, vocabulary: vocabulary, appContext: appContext, app: app?(),
                                                   package: package, canClear: canClear, recent: recent, history: history)
                    let (bytes, response) = try await self.session.bytes(for: request)
                    guard let response = response as? HTTPURLResponse else { throw TalkError("Correction is unavailable.") }
                    if [408, 504].contains(response.statusCode) { throw URLError(.timedOut) }
                    guard (200..<300).contains(response.statusCode) else {
                        throw TalkError("Correction proxy returned HTTP \(response.statusCode).")
                    }
                    let (action, items) = try await Self.action(bytes, package: package, canClear: canClear)
                    try Task.checkCancellation()
                    guard let action else {
                        guard let last else { throw TalkError("Correction returned no tool call.") }
                        guard last != .listThreads else { throw TalkError("No matching T3 thread was selected.") }
                        completion(.success(last)); return
                    }
                    guard let perform else { completion(.success(action)); return }
                    guard count < 8 else { throw TalkError("Stopped after eight tool calls.") }
                    guard let call = items.first(where: { $0["type"] as? String == "function_call" }),
                          let id = call["call_id"] as? String, !id.isEmpty,
                          !history.contains(where: { $0["call_id"] as? String == id }) else { throw TalkError("Invalid tool call ID.") }
                    perform(action) { output in
                        if action == .nextThread { completion(.success(action)); return }
                        if case .slack = action { completion(.success(action)); return }
                        if case .paste = action { completion(.success(action)); return }
                        turn(history + items + [["type": "function_call_output", "call_id": id, "output": output]],
                             last: action, count: count + 1)
                    }
                } catch let error as URLError where error.code == .timedOut {
                    completion(.failure(CorrectionTimeout { turn(history, last: last, count: count) }))
                } catch { completion(.failure(error)) }
            }
        }
        if package == .t3, let action = T3Tools.directSwitch(text, recent: recent) {
            if let perform { perform(action) { _ in completion(.success(action)) } }
            else { completion(.success(action)) }
        } else { turn([], last: nil, count: 0) }
    }
    func cancel() { task?.cancel(); task = nil }
    deinit { task?.cancel() }
}
