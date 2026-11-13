import Foundation
import TalkCore

/// Authenticates on the thread’s host, then uses its protocol-2 API.
actor T3Server {
    static let shared = T3Server()
    private let home = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".t3")
    private var credential: (environment: String, origin: URL, token: String, expires: Date)?

    private func request(_ path: String, environment: String) async throws -> URLRequest {
        let (base, token) = try await connection(environment)
        guard let url = URL(string: path, relativeTo: base)?.absoluteURL else { throw TalkError("Invalid T3 request.") }
        var request = URLRequest(url: url, timeoutInterval: 5)
        request.setValue("Bearer " + token, forHTTPHeaderField: "Authorization")
        request.setValue("2", forHTTPHeaderField: "x-t3-orchestration-protocol")
        return request
    }

    private func connection(_ environment: String) async throws -> (URL, String) {
        let localID = try String(contentsOf: home.appendingPathComponent("userdata/environment-id"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
        let base: URL, binary: URL, arguments: [String]
        let issue = ["auth", "session", "issue", "--scope", "orchestration:read", "--scope", "orchestration:operate", "--ttl", "5m", "--label", "HeyTalk", "--token-only"]
        if environment == localID {
            let runtime = try JSONSerialization.jsonObject(with: Data(contentsOf: home.appendingPathComponent("userdata/server-runtime.json"))) as? [String: Any]
            guard let origin = runtime?["origin"] as? String, let url = URL(string: origin),
                  url.scheme == "http", ["localhost", "127.0.0.1", "::1"].contains(url.host ?? "") else { throw TalkError("T3’s local server is unavailable.") }
            base = url; binary = home.appendingPathComponent("bin/t3"); arguments = issue
        } else {
            var match: (T3Tunnel, String)?
            for tunnel in T3Tunnel.connected() {
                let probe = URLRequest(url: tunnel.origin.appendingPathComponent(".well-known/t3/environment"), timeoutInterval: 1)
                guard let (data, response) = try? await URLSession.shared.data(for: probe), (response as? HTTPURLResponse)?.statusCode == 200,
                      let info = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      info["environmentId"] as? String == environment, let version = info["serverVersion"] as? String,
                      version.range(of: #"\A[0-9][A-Za-z0-9.+-]*\z"#, options: .regularExpression) != nil else { continue }
                match = (tunnel, version); break
            }
            guard let (tunnel, version) = match else { throw TalkError("Reconnect this thread’s SSH environment in T3 and try again.") }
            base = tunnel.origin; binary = URL(fileURLWithPath: "/usr/bin/ssh")
            arguments = ["-T", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "-p", String(tunnel.sshPort), "--", tunnel.host,
                         "exec ~/.t3/runtime/versions/" + version + "/t3 " + issue.joined(separator: " ")]
        }
        if let credential, credential.environment == environment, credential.origin == base, credential.expires > Date() { return (base, credential.token) }
        let token = try await Task.detached { try Self.issueToken(binary, arguments: arguments) }.value
        credential = (environment, base, token, Date().addingTimeInterval(240))
        return (base, token)
    }

    private static func issueToken(_ binary: URL, arguments: [String]) throws -> String {
        let process = Process(), output = Pipe()
        process.executableURL = binary
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = output; process.standardError = FileHandle.nullDevice
        try process.run()
        let timeout = DispatchWorkItem {
            if process.isRunning {
                process.terminate()
                DispatchQueue.global().asyncAfter(deadline: .now() + 1) {
                    if process.isRunning { kill(process.processIdentifier, SIGKILL) }
                }
            }
        }
        DispatchQueue.global().asyncAfter(deadline: .now() + 12, execute: timeout)
        defer { timeout.cancel() }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        let token = String(decoding: data, as: UTF8.self).split(whereSeparator: \.isNewline).last.map(String.init) ?? ""
        guard process.terminationStatus == 0, !token.isEmpty, token.count < 16_384 else {
            throw TalkError("Couldn’t authenticate with T3. Open T3 to refresh its CLI.")
        }
        return token
    }

    private func get(_ path: String, environment: String) async throws -> [String: Any] {
        var request = try await request(path, environment: environment)
        if path.contains("/history?") { request.timeoutInterval = 30 }
        let (data, response) = try await URLSession.shared.data(for: request)
        guard (response as? HTTPURLResponse)?.statusCode == 200,
              let value = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw TalkError("T3 could not read the thread.") }
        return value
    }

    func catchUp(_ id: String, environment: String) async throws -> SpokenRecap {
        guard UUID(uuidString: id) != nil else { throw TalkError("Invalid T3 thread.") }
        let path = "/api/orchestration/threads/\(id)"
        var page = try await get(path + "/bounded?deferWork=1", environment: environment)
        guard let projection = page["projection"] as? [String: Any],
              (projection["thread"] as? [String: Any])?["id"] as? String == id,
              var items = projection["visibleTurnItems"] as? [[String: Any]] else { throw TalkError("T3 could not read this thread.") }
        var newer = [String](), seen = Set<String>()
        var cursor = page["historyCursor"] as? String
        while true {
            if let passage = try Self.catchUpText(items, newer: &newer) { return passage }
            guard page["hasMoreHistory"] as? Bool == true else { throw TalkError("No message from you in this thread yet.") }
            guard let next = cursor, seen.insert(next).inserted else { throw TalkError("T3 could not read earlier messages.") }
            try Task.checkCancellation()
            var query = URLComponents()
            query.queryItems = [URLQueryItem(name: "cursor", value: next), URLQueryItem(name: "view", value: "conversation")]
            page = try await get(path + "/history?" + query.percentEncodedQuery!, environment: environment)
            guard let rows = page["items"] as? [[String: Any]] else { throw TalkError("T3 could not read earlier messages.") }
            items = rows; cursor = page["nextCursor"] as? String
        }
    }

    static func catchUpText(_ items: [[String: Any]], newer replies: inout [String]) throws -> SpokenRecap? {
        for row in items.reversed() {
            guard let item = row["item"] as? [String: Any] else { continue }
            if item["type"] as? String == "user_message", (item["createdBy"] as? String ?? "user") == "user" {
                guard !replies.isEmpty else { throw TalkError("No replies since your last message.") }
                return SpokenRecap(lastMessage: item["text"] as? String ?? "", replies: replies.reversed().joined(separator: "\n\n"))
            }
            if item["type"] as? String == "assistant_message",
               let text = item["text"] as? String, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { replies.append(text) }
        }
        return nil
    }

    func threads(environment: String) async throws -> [[String: Any]] {
        let snapshot = try await get("/api/orchestration/shell?omitIdleSubagents=1&omitOlderSettled=1", environment: environment)
        guard let threads = snapshot["threads"] as? [[String: Any]] else { throw TalkError("T3 returned no thread list.") }
        return threads.filter { ($0["lineage"] as? [String: Any])?["relationshipToParent"] as? String != "subagent" }
    }

    func thread(_ id: String, environment: String) async throws -> [String: Any] {
        guard UUID(uuidString: id) != nil else { throw TalkError("Invalid T3 thread ID.") }
        let snapshot = try await get("/api/orchestration/threads/\(id)/bounded?deferWork=1&compactTurnItems=1", environment: environment)
        guard let projection = snapshot["projection"] as? [String: Any], let thread = projection["thread"] as? [String: Any],
              thread["id"] as? String == id else { throw TalkError("T3 returned a different thread.") }
        return thread
    }

    func apply(_ action: CorrectionAction, thread id: String, environment: String, isCurrent: @escaping @MainActor () -> Bool) async throws {
        let before = try await thread(id, environment: environment)
        guard let command = try Self.command(action, thread: before) else { return }
        var request = try await request("/ws?orchestrationProtocol=2", environment: environment)
        var components = URLComponents(url: request.url!, resolvingAgainstBaseURL: false)!
        components.scheme = "ws"; request.url = components.url
        let socket = URLSession.shared.webSocketTask(with: request)
        let timeout = DispatchWorkItem { socket.cancel(with: .goingAway, reason: nil) }
        DispatchQueue.global().asyncAfter(deadline: .now() + 5, execute: timeout)
        defer { timeout.cancel(); socket.cancel(with: .normalClosure, reason: nil) }
        let payload = try JSONSerialization.data(withJSONObject: ["_tag": "Request", "id": "1", "tag": "orchestration.dispatchCommand", "payload": command, "headers": []])
        guard await isCurrent() else { throw CancellationError() }
        socket.resume()
        try await socket.send(.data(payload))
        while true {
            let message = try await socket.receive()
            let data: Data
            switch message { case .data(let bytes): data = bytes; case .string(let text): data = Data(text.utf8); @unknown default: continue }
            guard let result = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { continue }
            if result["_tag"] as? String == "Ping" { try await socket.send(.string("{\"_tag\":\"Pong\"}")); continue }
            guard result["requestId"] as? String == "1" else { continue }
            guard let exit = result["exit"] as? [String: Any], exit["_tag"] as? String == "Success" else { throw TalkError("T3 rejected the thread action.") }
            break
        }
        // Confirm server state, without toggling or retrying a mutation.
        let after = try await thread(id, environment: environment)
        guard try Self.command(action, thread: after) == nil else { throw TalkError("T3 did not apply the requested thread state.") }
    }

    static func command(_ action: CorrectionAction, thread: [String: Any]) throws -> [String: Any]? {
        guard let id = thread["id"] as? String, UUID(uuidString: id) != nil else { throw TalkError("Invalid T3 thread ID.") }
        var command: [String: Any] = ["commandId": UUID().uuidString, "threadId": id]
        switch action {
        case .pinThread(let pinned):
            if (thread["pinnedAt"] as? String != nil) == pinned { return nil }
            command["type"] = pinned ? "thread.pin" : "thread.unpin"
        case .settleThread:
            if thread["settledOverride"] as? String == "settled", thread["pinnedAt"] as? String == nil { return nil }
            command["type"] = "thread.settle"
        case .rename(let title):
            if thread["title"] as? String == title { return nil }
            command["type"] = "thread.metadata.update"; command["title"] = title
        default: throw TalkError("Unavailable T3 server action.")
        }
        return command
    }
}
