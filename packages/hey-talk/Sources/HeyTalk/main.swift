import Foundation
import AppKit
import TalkCore
import Darwin

signal(SIGPIPE, SIG_IGN)
let arguments = Array(CommandLine.arguments.dropFirst())
do {
    if arguments.first == "--worker" {
        guard arguments.count == 3 else { throw TalkError("Invalid worker arguments") }
        try runInferenceWorker(model: arguments[1], library: arguments[2])
    } else if arguments.contains("--serve") || (arguments.isEmpty && Bundle.main.bundleURL.pathExtension == "app") {
        let application = NSApplication.shared
        let delegate = TalkService(headless: arguments.contains("--headless"))
        application.delegate = delegate
        withExtendedLifetime(delegate) { application.run() }
    } else { try CLI.run(arguments) }
} catch {
    if arguments.first == "--worker" {
        try? Wire.write(Wire.json(["error": error.localizedDescription]), to: STDOUT_FILENO)
    }
    fputs("hey-talk: \(error.localizedDescription)\n", stderr)
    exit(1)
}
