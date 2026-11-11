import Foundation
import AppKit
import TalkCore
import Darwin

signal(SIGPIPE, SIG_IGN)
let arguments = Array(CommandLine.arguments.dropFirst())
do {
    if arguments.first == "--speech-worker" {
        guard arguments.count == 1 || arguments.count == 4 else { throw TalkError("Invalid speech worker arguments") }
        guard let model = SpeechModel.selected else { throw TalkError("Reinstall Hey Talk to restore the speech model manifest.") }
        try runSpeechWorker(model: arguments.count == 4 ? arguments[1] : model.directory.path, voice: model.voice,
            library: arguments.count == 4 ? arguments[2] : ModelPaths.library.path,
            alignmentModel: arguments.count == 4 ? arguments[3] : ModelPaths.directory.path)
    } else if arguments.first == "--worker" {
        guard arguments.count == 4 else { throw TalkError("Invalid worker arguments") }
        try runInferenceWorker(model: arguments[1], library: arguments[2], vocabulary: arguments[3])
    } else if arguments.contains("--serve") || (arguments.isEmpty && Bundle.main.bundleURL.pathExtension == "app") {
        let application = NSApplication.shared
        let delegate = TalkService(headless: arguments.contains("--headless"))
        application.delegate = delegate
        withExtendedLifetime(delegate) { application.run() }
    } else { try CLI.run(arguments) }
} catch {
    if ["--worker", "--speech-worker"].contains(arguments.first ?? "") {
        try? Wire.write(Wire.json(["error": error.localizedDescription]), to: STDOUT_FILENO)
    }
    fputs("hey-talk: \(error.localizedDescription)\n", stderr)
    exit(1)
}
