import AppKit
import TalkCore

if CommandLine.arguments.count > 1 {
    do { exit(try runCLI(Array(CommandLine.arguments.dropFirst()))) }
    catch {
        if let data = try? Wire.json(["ok": false, "error": error.localizedDescription]) { FileHandle.standardOutput.write(data) }
        exit(1)
    }
}
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
do {
    let delegate = try ClipboardApp(history: History())
    try delegate.server.start()
    app.delegate = delegate
    app.run()
} catch {
    NSLog("Hey Clipboard: %@", error.localizedDescription)
    exit(1)
}
