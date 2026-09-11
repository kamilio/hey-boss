import Foundation
import Darwin

func run(_ executable: String, _ arguments: [String]) {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    try! process.run()
    process.waitUntilExit()
    precondition(process.terminationStatus == 0)
}

func installHeyBoss() -> [String: String] {
    let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
    precondition(ProcessInfo.processInfo.operatingSystemVersion.majorVersion >= 26, "hey-boss requires macOS 26 or later")
    let environment = ProcessInfo.processInfo.environment
    let state = URL(fileURLWithPath: environment["HEY_BOSS_STATE_DIR"]!)
    let binaries = URL(fileURLWithPath: environment["HEY_BOSS_BIN_DIR"]!)
    let agents = URL(fileURLWithPath: environment["HEY_BOSS_LAUNCH_AGENTS_DIR"]!)
    let files = FileManager.default
    for directory in [state, binaries, agents, root.appendingPathComponent("out")] {
        try! files.createDirectory(at: directory, withIntermediateDirectories: true)
    }
    try! files.setAttributes([.posixPermissions: 0o700], ofItemAtPath: state.path)
    let staging = root.appendingPathComponent("out/hey-boss-daemon")
    run("/usr/bin/env", ["cargo", "build", "--locked", "--release", "--manifest-path", root.appendingPathComponent("Cargo.toml").path])
    run("/usr/bin/xcrun", ["swiftc", "-O", "-whole-module-optimization", "-parse-as-library", root.appendingPathComponent("hey_boss_daemon.swift").path, "-o", staging.path])
    let binary = binaries.appendingPathComponent("hey-boss")
    let daemon = state.appendingPathComponent("hey-boss-daemon")
    try! Data(contentsOf: root.appendingPathComponent("target/release/hey-boss")).write(to: binary, options: .atomic)
    try! Data(contentsOf: staging).write(to: daemon, options: .atomic)
    for executable in [binary, daemon] { try! files.setAttributes([.posixPermissions: 0o755], ofItemAtPath: executable.path) }
    try! files.removeItem(at: staging)
    let setup = root.appendingPathComponent("out/hey-boss-setup")
    run("/usr/bin/xcrun", ["swiftc", "-O", root.appendingPathComponent("setup_hey_boss.swift").path, "-o", setup.path])
    run(setup.path, [state.path, binaries.path, agents.path, daemon.path])
    try! files.removeItem(at: setup)
    let plist = agents.appendingPathComponent("local.hey-boss.plist")
    return ["binary": binary.path, "daemon": daemon.path, "launch_agent": plist.path]
}

let arguments = Array(CommandLine.arguments.dropFirst())
precondition(arguments.count <= 1 && arguments.allSatisfy { ["--json", "--markdown"].contains($0) })
let result = installHeyBoss()
if arguments.contains("--json") {
    print(String(data: try! JSONSerialization.data(withJSONObject: result), encoding: .utf8)!)
} else {
    for key in result.keys.sorted() { print("\(key): \(result[key]!)") }
}
