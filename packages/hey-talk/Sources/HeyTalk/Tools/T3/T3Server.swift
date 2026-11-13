import Foundation
import TalkCore

actor T3Server {
    static let shared = T3Server()

    private func run(_ args: [String], isCurrent: (@MainActor () -> Bool)? = nil) async throws -> Data {
        let task = Process(), input = Pipe(), output = Pipe()
        let installed = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/bin/hey-t3")
        task.executableURL = Bundle.main.url(forResource: "hey-t3", withExtension: "mjs") ?? installed
        task.arguments = (isCurrent == nil ? [] : ["--check-focus"]) + args
        var env = ProcessInfo.processInfo.environment
        env["PATH"] = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/bin").path + ":/opt/homebrew/bin:/usr/local/bin:" + (env["PATH"] ?? "/usr/bin:/bin")
        task.environment = env
        task.standardInput = input; task.standardOutput = output; task.standardError = output
        try task.run()
        defer { if task.isRunning { task.terminate() }; try? input.fileHandleForWriting.close() }
        var last = ""
        for try await line in output.fileHandleForReading.bytes.lines {
            if line == "READY", let isCurrent {
                let current = await isCurrent()
                try input.fileHandleForWriting.write(contentsOf: Data((current ? "GO\n" : "STOP\n").utf8))
            } else { last = line }
        }
        task.waitUntilExit()
        guard task.terminationStatus == 0 else { throw TalkError(last.isEmpty ? "T3 command failed." : last) }
        return Data(last.utf8)
    }

    func catchUp(_ id: String, environment: String) async throws -> SpokenRecap {
        try await JSONDecoder().decode(SpokenRecap.self, from: run(["catch-up", "t3code://threads/\(environment)/\(id)"]))
    }

    func threads(environment: String) async throws -> [[String: Any]] {
        guard let threads = try await JSONSerialization.jsonObject(with: run(["list", environment])) as? [[String: Any]] else { throw TalkError("T3 returned no thread list.") }
        return threads
    }

    func thread(_ id: String, environment: String) async throws -> [String: Any] {
        guard let thread = try await JSONSerialization.jsonObject(with: run(["show", "t3code://threads/\(environment)/\(id)"])) as? [String: Any] else { throw TalkError("T3 returned no thread.") }
        return thread
    }

    func apply(_ action: CorrectionAction, thread id: String, environment: String, isCurrent: @escaping @MainActor () -> Bool) async throws {
        let link = "t3code://threads/\(environment)/\(id)"
        let args: [String]
        switch action {
        case .rename(let title): args = ["rename", link, title]
        case .pinThread(let pin): args = [pin ? "pin" : "unpin", link]
        case .settleThread: args = ["settle", link]
        default: throw TalkError("Unavailable T3 action.")
        }
        _ = try await run(args, isCurrent: isCurrent)
    }
}
