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

// The existing loopback proxy owns OpenAI credentials. Audio never leaves the Mac.
final class Correction {
    private let session: URLSession
    private var task: Task<Void, Never>?

    init(session: URLSession = URLSession(configuration: {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForResource = 8
        configuration.connectionProxyDictionary = [:]
        return configuration
    }())) { self.session = session }

    static func request(text: String, vocabulary: [String], appContext: String? = nil) throws -> URLRequest {
        let bundle = Bundle.main.bundleURL.pathExtension == "app" ? Bundle.main : Bundle.module
        guard let url = bundle.url(forResource: "correction", withExtension: "md") else {
            throw TalkError("Correction prompt is missing. Reinstall Hey Talk.")
        }
        let prompt = try String(contentsOf: url, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !prompt.isEmpty else { throw TalkError("Correction prompt is empty.") }
        var request = URLRequest(url: URL(string: "http://127.0.0.1:8080/v1/responses")!, timeoutInterval: 8)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        var values: [String: Any] = ["transcript": text, "vocabulary": vocabulary]
        if let appContext { values["app_context"] = appContext }
        let input = try JSONSerialization.data(withJSONObject: values, options: [.sortedKeys])
        request.httpBody = try JSONSerialization.data(withJSONObject: [
            "model": "gpt-6-luna", "service_tier": "priority", "reasoning": ["effort": "none"], "store": false, "stream": true,
            "instructions": prompt, "input": String(decoding: input, as: UTF8.self)
        ])
        return request
    }

    private static func correctedText(_ bytes: URLSession.AsyncBytes) async throws -> String {
        var text = ""
        for try await line in bytes.lines where line.hasPrefix("data:") {
            let data = Data(line.dropFirst(5).utf8)
            guard let event = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
                throw TalkError("Invalid correction event.")
            }
            switch event["type"] as? String {
            case "response.output_text.delta":
                guard let delta = event["delta"] as? String else { throw TalkError("Invalid correction text.") }
                text += delta
            case "response.completed":
                let result = text.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !result.isEmpty else { throw TalkError("Correction returned no text.") }
                return result
            case "error", "response.failed", "response.incomplete":
                throw TalkError("Correction did not finish.")
            default: break
            }
        }
        throw TalkError("Correction did not finish.")
    }

    func correct(_ text: String, vocabulary: [String], appContext: String? = nil, completion: @escaping (Result<String, Error>) -> Void) {
        task = Task { [session] in
            let result: Result<String, Error>
            do {
                let request = try Self.request(text: text, vocabulary: vocabulary, appContext: appContext)
                let (bytes, response) = try await session.bytes(for: request)
                guard let response = response as? HTTPURLResponse else { throw TalkError("Correction is unavailable.") }
                guard (200..<300).contains(response.statusCode) else {
                    throw TalkError("Correction proxy returned HTTP \(response.statusCode).")
                }
                result = .success(try await Self.correctedText(bytes))
            } catch { result = .failure(error) }
            await MainActor.run { completion(result) }
        }
    }
    func cancel() { task?.cancel(); task = nil }
    deinit { task?.cancel() }
}
