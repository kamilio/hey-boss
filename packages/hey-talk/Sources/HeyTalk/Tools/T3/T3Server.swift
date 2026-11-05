import Foundation
import TalkCore

/// Uses T3's local CLI for short-lived authentication, then its protocol-2 API.
actor T3Server {
    static let shared = T3Server()
    private let home = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".t3")
    private var credential: (token: String, expires: Date)?

    private func request(_ path: String, environment: String) async throws -> URLRequest {
        let localID = try String(contentsOf: home.appendingPathComponent("userdata/environment-id"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
        guard environment == localID else { throw TalkError("This T3 thread is on another environment.") }
        let runtime = try JSONSerialization.jsonObject(with: Data(contentsOf: home.appendingPathComponent("userdata/server-runtime.json"))) as? [String: Any]
        guard let origin = runtime?["origin"] as? String, let base = URL(string: origin),
              base.scheme == "http", ["localhost", "127.0.0.1", "::1"].contains(base.host ?? ""),
              let url = URL(string: path, relativeTo: base)?.absoluteURL else { throw TalkError("T3's local server is unavailable.") }
        if credential == nil || credential!.expires <= Date() {
            let binary = home.appendingPathComponent("bin/t3")
            let token = try await Task.detached { try Self.issueToken(binary) }.value
            credential = (token, Date().addingTimeInterval(240))
        }
        var request = URLRequest(url: url, timeoutInterval: 5)
        request.setValue("Bearer " + credential!.token, forHTTPHeaderField: "Authorization")
        request.setValue("2", forHTTPHeaderField: "x-t3-orchestration-protocol")
        return request
    }

    private static func issueToken(_ binary: URL) throws -> String {
        let process = Process(), output = Pipe()
        process.executableURL = binary
        process.arguments = ["auth", "session", "issue", "--scope", "orchestration:read", "--scope", "orchestration:operate",
                             "--ttl", "5m", "--label", "Hey Talk", "--token-only"]
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
        DispatchQueue.global().asyncAfter(deadline: .now() + 5, execute: timeout)
        defer { timeout.cancel() }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        let token = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        guard process.terminationStatus == 0, !token.isEmpty, token.count < 16_384 else {
            throw TalkError("Couldn’t authenticate with T3. Open T3 to refresh its CLI.")
        }
        return token
    }

    private func get(_ path: String, environment: String) async throws -> [String: Any] {
        let request = try await request(path, environment: environment)
        let (data, response) = try await URLSession.shared.data(for: request)
        guard (response as? HTTPURLResponse)?.statusCode == 200,
              let value = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw TalkError("T3 could not read the thread.") }
        return value
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
