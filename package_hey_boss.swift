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
var resolvedAgentRevision: String?
func agentRevision() throws -> String {
    if let revision = resolvedAgentRevision { return revision }
    let git = Process(), output = Pipe()
    git.executableURL = URL(fileURLWithPath: "/usr/bin/git")
    git.arguments = ["ls-remote", "https://github.com/poe-platform/poe-code.git", "refs/heads/main"]
    git.standardOutput = output
    try git.run()
    let text = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
    git.waitUntilExit()
    let revision = String(text.split(whereSeparator: \.isWhitespace).first ?? "")
    require(git.terminationStatus == 0 && revision.count == 40 && revision.allSatisfy { $0.isHexDigit }, "Could not resolve Poe Agent’s GitHub revision")
    resolvedAgentRevision = revision; return revision
}
func desktopBuild() throws -> String {
    var hash = SHA256()
    // CLI changes must not rewrite the app. Include every desktop packaging input.
    for name in ["hey_boss_daemon.swift", "package_hey_boss.swift", "Cargo.toml", "assets/hey-boss.icns", "assets/conversation.md", "assets/conversation-tools.md", "assets/conversation-agent.mjs", "assets/build-conversation-agent.mjs", "assets/signing-identity.sha1"] {
        hash.update(data: Data(name.utf8)); hash.update(data: Data([0]))
        hash.update(data: try Data(contentsOf: root.appendingPathComponent(name)))
        hash.update(data: Data([0]))
    }
    hash.update(data: Data(try agentRevision().utf8))
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
func signInDesktopSession(_ arguments: [String], beside path: URL) throws -> Int32 {
    // An SSH audit session cannot use the desktop's unlocked login keychain.
    // A one-shot GUI job uses the existing codesign ACL without unlocking keys.
    let label = "local.hey-boss.signing.\(UUID().uuidString)"
    let directory = path.deletingLastPathComponent().appendingPathComponent(".signing-\(UUID().uuidString)")
    try files.createDirectory(at: directory, withIntermediateDirectories: false,
        attributes: [.posixPermissions: 0o700])
    let domain = "gui/\(getuid())"
    let log = directory.appendingPathComponent("stderr")
    defer {
        let stop = Process(); stop.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        stop.arguments = ["bootout", "\(domain)/\(label)"]
        stop.standardOutput = FileHandle.nullDevice; stop.standardError = FileHandle.nullDevice
        if (try? stop.run()) != nil { stop.waitUntilExit() }
        if let data = try? Data(contentsOf: log) { FileHandle.standardError.write(data) }
        try? files.removeItem(at: directory)
    }
    let plist = directory.appendingPathComponent("job.plist")
    let job: [String: Any] = ["Label": label, "ProgramArguments": ["/usr/bin/codesign"] + arguments,
        "RunAtLoad": true, "StandardErrorPath": log.path]
    try PropertyListSerialization.data(fromPropertyList: job, format: .xml, options: 0).write(to: plist)
    let start = Process(); start.executableURL = URL(fileURLWithPath: "/bin/launchctl")
    start.arguments = ["bootstrap", domain, plist.path]
    try start.run(); start.waitUntilExit()
    guard start.terminationStatus == 0 else { return start.terminationStatus }
    let deadline = Date().addingTimeInterval(30)
    while Date() < deadline {
        let status = Process(); status.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        status.arguments = ["print", "\(domain)/\(label)"]
        let output = Pipe(); status.standardOutput = output; status.standardError = FileHandle.nullDevice
        try status.run()
        let state = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        status.waitUntilExit()
        guard status.terminationStatus == 0 else { return status.terminationStatus }
        for line in state.components(separatedBy: .newlines) {
            let line = line.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("last exit code = "), let code = Int32(line.dropFirst("last exit code = ".count)) {
                return code
            }
        }
        Thread.sleep(forTimeInterval: 0.1)
    }
    FileHandle.standardError.write(Data("Desktop signing timed out; check the Mac for a keychain access prompt.\n".utf8))
    return 1
}
func sign(_ path: URL, identifier: String) throws {
    let arguments = ["--force", "--sign", identity, "--identifier", identifier, "--timestamp=none", path.path]
    let status: Int32
    if ProcessInfo.processInfo.environment["SSH_CONNECTION"] != nil {
        status = try signInDesktopSession(arguments, beside: path)
    } else {
        let sign = Process(); sign.executableURL = URL(fileURLWithPath: "/usr/bin/codesign")
        sign.arguments = arguments
        try sign.run(); sign.waitUntilExit()
        status = sign.terminationStatus
    }
    require(status == 0, "Could not sign \(path.path) with the pinned identity")
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
try Data(contentsOf: root.appendingPathComponent("assets/conversation.md"))
    .write(to: resources.appendingPathComponent("conversation.md"), options: .atomic)
let buildAgent = Process()
buildAgent.executableURL = URL(fileURLWithPath: "/usr/bin/env")
buildAgent.arguments = ["node", root.appendingPathComponent("assets/build-conversation-agent.mjs").path, resources.path, try agentRevision()]
var environment = ProcessInfo.processInfo.environment
environment["PATH"] = files.homeDirectoryForCurrentUser.path + "/.local/bin:/opt/homebrew/bin:/usr/local/bin:" + (environment["PATH"] ?? "/usr/bin:/bin")
buildAgent.environment = environment
try buildAgent.run(); buildAgent.waitUntilExit()
require(buildAgent.terminationStatus == 0, "Could not build Poe Agent from GitHub")
try sign(resources.appendingPathComponent("node"), identifier: "local.hey-boss.agent-node")
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
    "NSMicrophoneUsageDescription": "Listen while you review a document or start a local voice conversation.",
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
