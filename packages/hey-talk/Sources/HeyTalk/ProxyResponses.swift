import Foundation
import TalkCore

enum ProxyResponses {
    static func request(profile: String? = nil) throws -> URLRequest {
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
        return request
    }
}
