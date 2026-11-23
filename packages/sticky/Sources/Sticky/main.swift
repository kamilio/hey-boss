import AppKit

var delegate: StickyApp?
var preview: NoteExport?
let arguments = Array(CommandLine.arguments.dropFirst())
do {
    if arguments == ["--help"] || arguments == ["help"] || arguments == ["-h"] {
        print(help)
    } else if arguments.isEmpty || arguments == ["run"] || arguments.first?.hasPrefix("-psn_") == true {
        if (try? request(["command":"show"])) == nil {
            let app = NSApplication.shared
            app.setActivationPolicy(.accessory)
            delegate = StickyApp(); app.delegate = delegate
            app.run()
        }
    } else if let (markdown, output, width, dark) = try CLI(arguments).run() {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        DispatchQueue.main.async {
            do {
                preview = try NoteExport(markdown: markdown, output: output, width: width, dark: dark) { result in
                    switch result {
                    case .success(let value): printJSON(value); exit(0)
                    case .failure(let error): fputs("Sticky: \(error.localizedDescription)\n", stderr); exit(1)
                    }
                }
            } catch { fputs("Sticky: \(error.localizedDescription)\n", stderr); exit(1) }
        }
        app.run()
    }
} catch {
    fputs("Sticky: \(error.localizedDescription)\n", stderr)
    exit(1)
}
