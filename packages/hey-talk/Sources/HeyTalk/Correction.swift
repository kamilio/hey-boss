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
    private var task: URLSessionDataTask?

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
            "model": "gpt-6-luna", "reasoning": ["effort": "none"], "store": false,
            "instructions": prompt, "input": String(decoding: input, as: UTF8.self)
        ])
        return request
    }

    static func text(from data: Data) throws -> String {
        guard let response = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              response["status"] as? String == "completed",
              let output = response["output"] as? [[String: Any]] else {
            throw TalkError("Correction did not finish.")
        }
        let text = output.filter { $0["type"] as? String == "message" && $0["role"] as? String == "assistant" }
            .flatMap { $0["content"] as? [[String: Any]] ?? [] }
            .filter { $0["type"] as? String == "output_text" }
            .compactMap { $0["text"] as? String }.joined()
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { throw TalkError("Correction returned no text.") }
        return text
    }

    func correct(_ text: String, vocabulary: [String], appContext: String? = nil, completion: @escaping (Result<String, Error>) -> Void) {
        do {
            let request = try Self.request(text: text, vocabulary: vocabulary, appContext: appContext)
            task = session.dataTask(with: request) { data, response, error in
                let result = Result<String, Error> {
                    if let error { throw error }
                    guard let response = response as? HTTPURLResponse, let data else {
                        throw TalkError("Correction is unavailable.")
                    }
                    guard (200..<300).contains(response.statusCode) else {
                        throw TalkError("Correction proxy returned HTTP \(response.statusCode).")
                    }
                    return try Self.text(from: data)
                }
                DispatchQueue.main.async { completion(result) }
            }
            task?.resume()
        } catch { completion(.failure(error)) }
    }
    func cancel() { task?.cancel(); task = nil }
    deinit { task?.cancel() }
}
