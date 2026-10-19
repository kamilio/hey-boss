import AppKit
import TalkCore
import Darwin

private var recordingInterrupted = false

enum CLI {
    static let help = """
    hey-talk — native, local dictation with Orukeet

    hey-talk start [--no-overlay] [--no-paste]   Start microphone capture
    hey-talk stop [--json]                      Transcribe and return the result
    hey-talk record [--no-overlay] [--no-paste]  Record until Enter or Ctrl-C
    hey-talk toggle                            Start or stop
    hey-talk cancel                            Discard audio; never paste
    hey-talk transcribe FILE [--json]           Transcribe an audio file locally
    hey-talk status [--json]                    State, audio levels, worker PID
    hey-talk last                              Last transcript, including after restart
    hey-talk history [--json]                  Latest 50 saved transcripts
    hey-talk history --before ID [--json]      Older saved transcripts
    hey-talk history show                      Open history with Copy buttons
    hey-talk devices                           List microphone names and UIDs
    hey-talk input UID|default                  Select microphone
    hey-talk overlay show|hide                  Control the floating recording pill
    hey-talk permissions microphone|accessibility
    hey-talk warm                              Load Orukeet ahead of recording
    hey-talk doctor                            Check permissions, model, and hotkey
    hey-talk quit                              Stop only Hey Talk and its worker

    Press \(DictationShortcut.hint) (Hyper+R) to start; press again to stop and paste. Esc cancels.
    Commands start the app on demand. No login service or account is required.
    Audio stays in memory. File transcription never modifies your clipboard.
    """

    static func run(_ arguments: [String]) throws {
        if arguments.isEmpty || arguments == ["--help"] || arguments == ["help"] { print(help); return }
        if arguments == ["--version"] { print("hey-talk 0.1.0"); return }
        let json = arguments.contains("--json")
        let noOverlay = arguments.contains("--no-overlay")
        let noPaste = arguments.contains("--no-paste")
        let args = arguments.filter { !["--json", "--no-overlay", "--no-paste"].contains($0) }
        guard !args.isEmpty else { throw TalkError("Missing command. Run hey-talk --help.") }
        let verb = args[0]
        let known = ["start", "stop", "toggle", "cancel", "record", "transcribe", "status", "last", "history", "devices", "input", "overlay", "permissions", "warm", "doctor", "quit"]
        guard known.contains(verb) else { throw TalkError("Unknown command: \(verb). Run hey-talk --help.") }
        let needsValue = ["transcribe", "input", "overlay", "permissions"].contains(verb)
        if verb != "history" {
            guard args.count == (needsValue ? 2 : 1) else { throw TalkError("Unexpected arguments. Run hey-talk --help.") }
        }
        var request: [String: Any] = ["command": verb]
        if verb == "history" {
            if args == ["history", "show"] { request["show"] = true }
            else if args.count == 3, args[1] == "--before", UUID(uuidString: args[2]) != nil { request["before"] = args[2] }
            else if args != ["history"] { throw TalkError("Use history [show | --before ID] [--json]") }
        }
        if noOverlay { request["overlay"] = false }
        if noPaste { request["paste"] = false }
        if verb == "transcribe" {
            request["file"] = URL(fileURLWithPath: (args[1] as NSString).expandingTildeInPath).standardizedFileURL.path
        }
        if verb == "input" { request["uid"] = args[1] }
        if verb == "overlay" {
            guard ["show", "hide"].contains(args[1]) else { throw TalkError("Use overlay show or overlay hide") }
            request["visible"] = args[1] == "show"
        }
        if verb == "permissions" { request["kind"] = args[1] }
        if (try? ControlSocket.request(["command": "status"])) == nil {
            if ["status", "quit", "cancel"].contains(verb) { try output(["ok": true, "state": "stopped"], json: json); return }
            try launch(headless: noOverlay || ["transcribe", "devices", "doctor", "input", "permissions", "warm", "history", "last"].contains(verb))
        }
        if verb == "record" {
            request["command"] = "start"
            let result = try ControlSocket.request(request)
            guard result["ok"] as? Bool == true else { throw TalkError(result["error"] as? String ?? "Could not record") }
            fputs("Recording. Press Enter or Ctrl-C to finish.\n", stderr)
            recordingInterrupted = false
            let previous = signal(SIGINT) { _ in recordingInterrupted = true }
            defer { signal(SIGINT, previous) }
            while !recordingInterrupted {
                var fd = pollfd(fd: STDIN_FILENO, events: Int16(POLLIN), revents: 0)
                if poll(&fd, 1, 100) > 0 {
                    var byte: UInt8 = 0
                    if read(STDIN_FILENO, &byte, 1) <= 0 || byte == 10 { break }
                }
            }
            try output(ControlSocket.request(["command": "stop"]), json: json)
        } else { try output(ControlSocket.request(request), json: json) }
    }
    private static func output(_ response: [String: Any], json: Bool) throws {
        if json {
            let data = try JSONSerialization.data(withJSONObject: response, options: [.sortedKeys])
            print(String(decoding: data, as: UTF8.self))
        } else if let text = response["text"] as? String { print(text) }
        else if let entries = response["entries"] as? [[String: Any]] {
            for entry in entries { print("\(entry["created_at"] ?? "")  \(entry["id"] ?? "")\n\(entry["text"] ?? "")\n") }
            if let next = response["next_before"] as? String { print("Older: hey-talk history --before \(next)") }
        }
        else if let devices = response["devices"] as? [[String: Any]] {
            for device in devices { print("\(device["default"] as? Bool == true ? "*" : " ") \(device["name"] ?? "")\t\(device["uid"] ?? "")") }
        } else if let error = response["error"] as? String { fputs("hey-talk: \(error)\n", stderr) }
        else {
            for key in response.keys.sorted() where key != "ok" { print("\(key): \(response[key]!)") }
        }
        if response["ok"] as? Bool == false { exit(1) }
    }
    private static func launch(headless: Bool) throws {
        let executable = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath()
        let adjacent = executable.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let candidates = [adjacent, URL(fileURLWithPath: "/Applications/Hey Talk.app"), FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Applications/Hey Talk.app")]
        guard let app = candidates.first(where: { $0.pathExtension == "app" && FileManager.default.fileExists(atPath: $0.appendingPathComponent("Contents/Info.plist").path) }) else {
            throw TalkError("Hey Talk.app is not installed. Build or install packages/hey-talk, then use the CLI inside its app bundle.")
        }
        // Launch Services gives microphone/TCC requests the app's own identity.
        // Direct Process() launch would attribute them to the calling terminal.
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = false
        configuration.arguments = headless ? ["--serve", "--headless"] : ["--serve"]
        var finished = false
        var failure: Error?
        NSWorkspace.shared.openApplication(at: app, configuration: configuration) { _, error in failure = error; finished = true }
        let deadline = Date().addingTimeInterval(15)
        while Date() < deadline {
            if finished, let failure { throw failure }
            if let fd = try? ControlSocket.connect() { close(fd); return }
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        }
        throw TalkError("Hey Talk did not start within 15 seconds")
    }
}
