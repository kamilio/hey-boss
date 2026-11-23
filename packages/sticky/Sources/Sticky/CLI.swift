import AppKit

let help = """
Sticky — floating Markdown notes

  sticky set FILE [--id ID]             Replace Daily Focus (ID: focus)
  sticky new [FILE]                     Create an additional note
  sticky get [--id ID]                  Print the saved Markdown
  sticky edit [--id ID]                 Open the large Markdown editor
  sticky list                          List notes as JSON
  sticky show|hide [--id ID]            Show/hide one note, or all notes
  sticky toggle                        Show/hide all notes (Hyper+N)
  sticky preview FILE --output PNG     Render a file without changing notes
  sticky export --output PNG [--id ID] Export a saved note
      [--width 400] [--appearance light|dark]
  sticky status                        App and shortcut status as JSON
  sticky quit                          Quit; refuses unsaved edits

Use FILE - to read Markdown from stdin. First # heading is the window title.
Preview/export writes a 2× PNG, including the full document (up to 8192 points).
Mermaid fences render charts and diagrams offline. Links open in their default app.
The note never takes focus. × hides it. The separate editor saves with ⌘S.
Hyper is Control+Command+Option+Shift. Sticky stays out of Command-Tab.
"""

func printJSON(_ value: [String: Any]) {
    if let data = try? JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys]), let text = String(data: data, encoding: .utf8) { print(text) }
}

func readSource(_ path: String) throws -> String {
    let data: Data
    if path == "-" {
        var input = Data()
        while input.count <= 1_048_576 {
            guard let chunk = try FileHandle.standardInput.read(upToCount: min(65536, 1_048_577 - input.count)), !chunk.isEmpty else { break }
            input.append(chunk)
        }
        data = input
    } else {
        let url = URL(fileURLWithPath: path)
        guard (try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0) <= 1_048_576 else { throw StickyError("Note exceeds 1 MiB.") }
        data = try Data(contentsOf: url)
    }
    guard data.count <= 1_048_576, let text = String(data: data, encoding: .utf8) else { throw StickyError("Notes must be UTF-8 text up to 1 MiB.") }
    _ = try RenderedDocument(text)
    return text
}

private func ensureRunning() throws -> Bool {
    if (try? request(["command":"status"])) != nil { return false }
    let binary = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath()
    let bundle = binary.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    let app = bundle.pathExtension == "app" ? bundle : FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Applications/Sticky.app")
    guard FileManager.default.fileExists(atPath: app.path) else { throw StickyError("Install Sticky first with packages/sticky/install.sh.") }
    let process = Process(); process.executableURL = URL(fileURLWithPath: "/usr/bin/open")
    process.arguments = ["-g", app.path]; try process.run(); process.waitUntilExit()
    guard process.terminationStatus == 0 else { throw StickyError("Could not launch Sticky.") }
    let deadline = Date().addingTimeInterval(8)
    while Date() < deadline {
        if (try? request(["command":"status"])) != nil { return true }
        Thread.sleep(forTimeInterval: 0.05)
    }
    throw StickyError("Sticky did not start within 8 seconds.")
}

struct CLI {
    let command: String
    let file: String?
    let options: [String: String]
    init(_ arguments: [String]) throws {
        command = arguments.first ?? "show"
        let allowed: [String: Set<String>] = [
            "set":["id"], "new":[], "get":["id"], "edit":["id"], "list":[], "show":["id"], "hide":["id"], "toggle":[],
            "preview":["output","width","appearance"], "export":["id","output","width","appearance"], "status":[], "quit":[]
        ]
        guard let flags = allowed[command] else { throw StickyError("Unknown command: \(command). Run sticky --help.") }
        var options: [String: String] = [:], file: String?
        var index = 1
        while index < arguments.count {
            let arg = arguments[index]
            if arg.hasPrefix("--") {
                let key = String(arg.dropFirst(2))
                guard flags.contains(key), index + 1 < arguments.count, options[key] == nil else { throw StickyError("Invalid option: \(arg)") }
                options[key] = arguments[index + 1]; index += 2
            } else {
                guard ["set","new","preview"].contains(command), file == nil else { throw StickyError("Unexpected argument: \(arg)") }
                file = arg; index += 1
            }
        }
        if ["set","preview"].contains(command), file == nil { throw StickyError("\(command) requires a Markdown file (or - for stdin).") }
        if ["export","preview"].contains(command) {
            guard let output = options["output"], URL(fileURLWithPath: output).pathExtension.lowercased() == "png" else { throw StickyError("Use --output with a .png path.") }
            if let width = options["width"], !(Double(width).map { $0.isFinite && (300...1200).contains($0) } ?? false) { throw StickyError("Width must be between 300 and 1200 points.") }
            if let appearance = options["appearance"], !["light","dark"].contains(appearance) { throw StickyError("Appearance must be light or dark.") }
        }
        self.file = file; self.options = options
    }
    func run() throws -> (String, URL, CGFloat, Bool)? {
        if command == "preview" {
            return (try readSource(file!), URL(fileURLWithPath: options["output"]!), CGFloat(Double(options["width"] ?? "400")!), options["appearance"] == "dark")
        }
        if command == "status" {
            printJSON((try? request(["command":"status"])) ?? ["running":false]); return nil
        }
        if ["quit", "hide"].contains(command), (try? request(["command":"status"])) == nil { printJSON(["ok":true]); return nil }
        // Validate all content before starting or mutating the app.
        let markdown = try file.map(readSource)
        let started = try ensureRunning()
        if command == "toggle", started { printJSON(["ok":true]); return nil }
        var message: [String: Any] = ["command": command == "export" ? "get" : command]
        if let id = options["id"] { message["id"] = id }
        if ["set","new"].contains(command) { message["markdown"] = markdown ?? "# Untitled\n\n" }
        let response = try request(message)
        if command == "export" {
            guard let source = response["markdown"] as? String else { throw StickyError("Missing note content.") }
            return (source, URL(fileURLWithPath: options["output"]!), CGFloat(Double(options["width"] ?? "400")!), options["appearance"] == "dark")
        }
        if command == "get" { print(response["markdown"] as? String ?? "", terminator: "") }
        else { printJSON(response) }
        return nil
    }
}
