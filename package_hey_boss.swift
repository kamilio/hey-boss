import Foundation

// Give Launch Services a real application identity, including before the first
// window opens. A runtime NSApplication icon on a bare executable is not enough.
precondition(CommandLine.arguments.count == 3, "Usage: swift package_hey_boss.swift DAEMON OUTPUT.app")
let files = FileManager.default
let source = URL(fileURLWithPath: CommandLine.arguments[1])
let app = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
precondition(app.pathExtension == "app")
let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let contents = app.appendingPathComponent("Contents", isDirectory: true)
let executables = contents.appendingPathComponent("MacOS", isDirectory: true)
let resources = contents.appendingPathComponent("Resources", isDirectory: true)
for directory in [executables, resources] {
    try files.createDirectory(at: directory, withIntermediateDirectories: true)
}
let executable = executables.appendingPathComponent("hey-boss-daemon")
try Data(contentsOf: source).write(to: executable, options: .atomic)
try files.setAttributes([.posixPermissions: 0o755], ofItemAtPath: executable.path)
try Data(contentsOf: root.appendingPathComponent("assets/hey-boss.icns"))
    .write(to: resources.appendingPathComponent("hey-boss.icns"), options: .atomic)
let manifest = try String(contentsOf: root.appendingPathComponent("Cargo.toml"), encoding: .utf8)
let version = manifest.components(separatedBy: .newlines).first { $0.hasPrefix("version = ") }!.components(separatedBy: "\"")[1]
let info: [String: Any] = [
    "CFBundleIdentifier": "local.hey-boss.desktop",
    "CFBundleName": "Hey Boss",
    "CFBundleDisplayName": "Hey Boss",
    "CFBundleExecutable": "hey-boss-daemon",
    "CFBundlePackageType": "APPL",
    "CFBundleInfoDictionaryVersion": "6.0",
    "CFBundleShortVersionString": version,
    "CFBundleVersion": version,
    "CFBundleIconFile": "hey-boss.icns",
    "LSMinimumSystemVersion": "26.0",
    "LSUIElement": true,
    "NSHighResolutionCapable": true,
]
try PropertyListSerialization.data(fromPropertyList: info, format: .xml, options: 0)
    .write(to: contents.appendingPathComponent("Info.plist"), options: .atomic)
try Data("APPL????".utf8).write(to: contents.appendingPathComponent("PkgInfo"), options: .atomic)
let sign = Process(); sign.executableURL = URL(fileURLWithPath: "/usr/bin/codesign")
sign.arguments = ["--force", "--sign", "-", app.path]
try sign.run(); sign.waitUntilExit(); precondition(sign.terminationStatus == 0)
print(app.path)
