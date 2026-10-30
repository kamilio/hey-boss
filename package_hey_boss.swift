import Foundation
import CryptoKit

func require(_ condition: Bool, _ message: String) {
    guard condition else {
        FileHandle.standardError.write(Data("\(message)\n".utf8))
        exit(1)
    }
}

// Give Launch Services a real application identity, including before the first
// window opens. A runtime NSApplication icon on a bare executable is not enough.
precondition(CommandLine.arguments.count == 3, "Usage: swift package_hey_boss.swift DAEMON OUTPUT.app | --cli BINARY | --check-desktop APP")
let files = FileManager.default
let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
// Keep the certificate stable across builds so macOS retains app permissions.
// Restore the shared local identity with packages/hey-talk/setup-signing.sh.
let identity = try String(contentsOf: root.appendingPathComponent("assets/signing-identity.sha1"), encoding: .utf8)
    .trimmingCharacters(in: .whitespacesAndNewlines)
precondition(identity.count == 40 && identity.allSatisfy { $0.isHexDigit }, "Invalid pinned signing identity")
func desktopBuild() throws -> String {
    var hash = SHA256()
    // CLI changes must not rewrite the app. Include every desktop packaging input.
    for name in ["hey_boss_daemon.swift", "package_hey_boss.swift", "Cargo.toml", "assets/hey-boss.icns", "assets/signing-identity.sha1"] {
        hash.update(data: Data(name.utf8)); hash.update(data: Data([0]))
        hash.update(data: try Data(contentsOf: root.appendingPathComponent(name)))
        hash.update(data: Data([0]))
    }
    return hash.finalize().map { String(format: "%02x", $0) }.joined()
}
func verify(_ path: URL, identifier: String) throws -> Bool {
    let verify = Process(); verify.executableURL = URL(fileURLWithPath: "/usr/bin/codesign")
    verify.arguments = ["--verify", "--deep", "--strict", "-R",
        "=identifier \"\(identifier)\" and certificate leaf = H\"\(identity)\"", path.path]
    verify.standardError = FileHandle.nullDevice
    try verify.run(); verify.waitUntilExit()
    return verify.terminationStatus == 0
}
if CommandLine.arguments[1] == "--check-desktop" {
    let app = URL(fileURLWithPath: CommandLine.arguments[2])
    let expected = try desktopBuild()
    let info = (try? Data(contentsOf: app.appendingPathComponent("Contents/Info.plist")))
        .flatMap { try? PropertyListSerialization.propertyList(from: $0, format: nil) as? [String: Any] }
    guard info?["HeyBossDesktopBuild"] as? String == expected else { exit(3) }
    exit(try verify(app, identifier: "local.hey-boss.desktop") ? 0 : 3)
}
let identities = Process()
let identityOutput = Pipe()
identities.executableURL = URL(fileURLWithPath: "/usr/bin/security")
identities.arguments = ["find-identity", "-p", "codesigning"]
identities.standardOutput = identityOutput
try identities.run()
let available = String(decoding: identityOutput.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
identities.waitUntilExit()
require(identities.terminationStatus == 0 && available.contains(identity),
    "Restore the persistent signing identity with packages/hey-talk/setup-signing.sh before packaging Hey Boss")
func sign(_ path: URL, identifier: String) throws {
    let sign = Process(); sign.executableURL = URL(fileURLWithPath: "/usr/bin/codesign")
    sign.arguments = ["--force", "--sign", identity, "--identifier", identifier, "--timestamp=none", path.path]
    try sign.run(); sign.waitUntilExit()
    require(sign.terminationStatus == 0, "Could not sign \(path.path) with the pinned identity")
    require(try verify(path, identifier: identifier), "Signing verification failed for \(path.path)")
}
// The installed CLI and temporary upgrade guard perform App Management work.
// Their identity must survive both rebuilds and changes to the executable name.
if CommandLine.arguments[1] == "--cli" {
    try sign(URL(fileURLWithPath: CommandLine.arguments[2]), identifier: "local.hey-boss.cli")
    exit(0)
}
let source = URL(fileURLWithPath: CommandLine.arguments[1])
let app = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
precondition(app.pathExtension == "app")
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
    "HeyBossDesktopBuild": try desktopBuild(),
    "CFBundleIconFile": "hey-boss.icns",
    "LSMinimumSystemVersion": "15.0",
    "NSHighResolutionCapable": true,
    "NSMicrophoneUsageDescription": "Hear your spoken comments while you review a document.",
    // The local Issues service intentionally uses HTTP. Keep ATS for all other hosts.
    "NSAppTransportSecurity": [
        "NSExceptionDomains": [
            "hey-boss.test": ["NSExceptionAllowsInsecureHTTPLoads": true],
        ],
    ],
]
try PropertyListSerialization.data(fromPropertyList: info, format: .xml, options: 0)
    .write(to: contents.appendingPathComponent("Info.plist"), options: .atomic)
try Data("APPL????".utf8).write(to: contents.appendingPathComponent("PkgInfo"), options: .atomic)
try sign(app, identifier: "local.hey-boss.desktop")
print(app.path)
