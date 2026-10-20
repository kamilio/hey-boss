import Foundation
import TalkCore
import Darwin

struct SharedVocabulary {
    let cache: Vocabulary
    let transport: ([String]) throws -> [String: Any]

    init(cache: Vocabulary = Vocabulary(), transport: @escaping ([String]) throws -> [String: Any] = SharedVocabulary.request) {
        self.cache = cache; self.transport = transport
    }

    func command(_ arguments: [String]) throws -> [String: Any] {
        try Vocabulary.validate(arguments)
        // The CLI and background refresh can run in separate processes. Keep a
        // delayed refresh from overwriting a more recent CLI result in the cache.
        let directory = cache.file.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
                                                attributes: [.posixPermissions: 0o700])
        let fd = open(directory.appendingPathComponent("vocabulary.lock").path, O_CREAT | O_RDWR | O_CLOEXEC, 0o600)
        guard fd >= 0 else { throw TalkError("Cannot lock vocabulary settings.") }
        defer { close(fd) }
        let deadline = Date().addingTimeInterval(15)
        while flock(fd, LOCK_EX | LOCK_NB) != 0 {
            guard errno == EWOULDBLOCK, Date() < deadline else { throw TalkError("Vocabulary settings are busy. Try again.") }
            usleep(10_000)
        }
        defer { flock(fd, LOCK_UN) }
        do {
            var response = try checked(transport([]))
            if response["initialized"] as? Bool == false {
                response = try checked(transport(["initialize", "--file", cache.prepare().path]))
            }
            if !arguments.isEmpty { response = try checked(transport(arguments)) }
            guard response["initialized"] as? Bool == true else { throw TalkError("Shared vocabulary is not initialized.") }
            response["vocabulary"] = try cache.replace(response["vocabulary"] as! [String])
            response["path"] = cache.file.path
            response["source"] = "fleet"
            return response
        } catch {
            // Dictation always has a local snapshot. Failed edits are never
            // reported as saved or silently placed in a second, private list.
            if !arguments.isEmpty { throw error }
            return ["ok": true, "vocabulary": try cache.command([]), "path": cache.file.path,
                    "source": "cache", "sync_error": error.localizedDescription]
        }
    }

    private func checked(_ response: [String: Any]) throws -> [String: Any] {
        guard response["ok"] as? Bool == true else {
            let detail = (response["error"] as? [String: Any])?["message"] as? String
            throw TalkError(detail ?? response["error"] as? String ?? "Could not read shared vocabulary.")
        }
        guard response["vocabulary"] is [String], response["initialized"] is Bool else {
            throw TalkError("Update Hey Boss to use shared vocabulary.")
        }
        return response
    }

    private static func request(_ arguments: [String]) throws -> [String: Any] {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let paths = (ProcessInfo.processInfo.environment["PATH"] ?? "").split(separator: ":").map(String.init)
            + [home + "/.local/bin", "/opt/homebrew/bin", "/usr/local/bin", home + "/.cargo/bin"]
        guard let binary = paths.map({ URL(fileURLWithPath: $0).appendingPathComponent("hey-boss") })
            .first(where: { FileManager.default.isExecutableFile(atPath: $0.path) }) else {
            throw TalkError("Install Hey Boss to share vocabulary across your machines.")
        }
        let process = Process(), output = Pipe()
        process.executableURL = binary
        process.arguments = ["settings", "vocabulary", "--json"] + arguments
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = output; process.standardError = FileHandle.nullDevice
        var environment = ProcessInfo.processInfo.environment
        environment.removeValue(forKey: "HEY_BOSS_ISSUE_HOST")
        environment.removeValue(forKey: "HEY_BOSS_ISSUE_PROJECT")
        process.environment = environment
        try process.run()
        let timeout = DispatchWorkItem {
            if process.isRunning {
                process.terminate()
                DispatchQueue.global().asyncAfter(deadline: .now() + 1) {
                    if process.isRunning { kill(process.processIdentifier, SIGKILL) }
                }
            }
        }
        DispatchQueue.global().asyncAfter(deadline: .now() + 10, execute: timeout)
        defer { timeout.cancel() }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard data.count <= 131_072,
              let response = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw TalkError("Hey Boss vocabulary is unavailable. Update Hey Boss and try again.")
        }
        return response
    }
}
