import Foundation
import TalkCore

/// Reuse wire's local Slack login; only the web token and Slack session cookie leave the Mac, directly to slack.com.
final class SlackAPI: NSObject, URLSessionTaskDelegate {
    private let token: String, cookie: String

    override init() {
        let url = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".wire/auth/slack_cookies.txt")
        let rows = (try? String(contentsOf: url, encoding: .utf8))?.components(separatedBy: .newlines).map { $0.components(separatedBy: "\t") } ?? []
        token = rows.first { $0.count == 3 && $0[0] == "# wire" && $0[1] == "token" }?.last ?? ""
        cookie = rows.first { $0.count == 7 && ["#HttpOnly_.slack.com", ".slack.com"].contains($0[0]) && $0[5] == "d" }?.last ?? ""
    }

    func call(_ method: String, _ params: [String: String]) async throws -> [String: Any] {
        guard !token.isEmpty, !cookie.isEmpty else { throw TalkError("Log in with wire slack login") }
        var form = URLComponents()
        form.queryItems = (params.merging(["token": token]) { _, new in new }).map { URLQueryItem(name: $0.key, value: $0.value) }
        var request = URLRequest(url: URL(string: "https://slack.com/api/" + method)!, timeoutInterval: 10)
        request.httpMethod = "POST"
        request.httpBody = Data((form.percentEncodedQuery ?? "").replacingOccurrences(of: "+", with: "%2B").utf8)
        request.httpShouldHandleCookies = false
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        request.setValue("d=" + cookie, forHTTPHeaderField: "Cookie")
        let (data, response) = try await URLSession.shared.data(for: request, delegate: self)
        guard (response as? HTTPURLResponse)?.statusCode == 200,
              let json = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw TalkError("Slack unavailable") }
        guard json["ok"] as? Bool == true else {
            switch json["error"] as? String {
            case "channel_not_found": throw TalkError("Chat not found")
            case "invalid_auth", "not_authed", "token_revoked": throw TalkError("Refresh Slack login")
            default: throw TalkError("Slack rejected the request")
            }
        }
        return json
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }
}
