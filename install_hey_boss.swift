import Foundation
import Darwin

func status(_ executable: String, _ arguments: [String]) -> Int32 {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    try! process.run()
    process.waitUntilExit()
    return process.terminationStatus
}

func run(_ executable: String, _ arguments: [String]) {
    precondition(status(executable, arguments) == 0)
}

// launchd pins each agent to the code signature it was registered with and
// refuses to spawn a replaced binary (EX_CONFIG) until the agent re-registers.
func reregisterAgents(in agents: URL, running replaced: Set<String>) {
    let domain = "gui/\(getuid())"
    for plist in try! FileManager.default.contentsOfDirectory(at: agents, includingPropertiesForKeys: nil) where plist.pathExtension == "plist" {
        guard let content = NSDictionary(contentsOf: plist),
              let program = (content["ProgramArguments"] as? [String])?.first ?? content["Program"] as? String,
              replaced.contains(program) else { continue }
        _ = status("/bin/launchctl", ["bootout", domain, plist.path])
        // bootstrap can fail with EIO while the bootout is still settling.
        var attempts = 0
        while status("/bin/launchctl", ["bootstrap", domain, plist.path]) != 0 {
            attempts += 1
            precondition(attempts < 5, "Cannot re-register \(plist.path)")
            sleep(1)
        }
    }
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
    let app = state.appendingPathComponent("Hey Boss.app")
    let daemon = app.appendingPathComponent("Contents/MacOS/hey-boss-daemon")
    try! Data(contentsOf: root.appendingPathComponent("target/release/hey-boss")).write(to: binary, options: .atomic)
    run("/usr/bin/swift", [root.appendingPathComponent("package_hey_boss.swift").path, staging.path, app.path])
    for executable in [binary, daemon] { try! files.setAttributes([.posixPermissions: 0o755], ofItemAtPath: executable.path) }
    var replaced = Set<String>()
    for name in ["hey-gh", "hey-harvester", "hey-proxy"] {
        let binary = binaries.appendingPathComponent(name)
        var paths = [binary]
        let cargo = files.homeDirectoryForCurrentUser.appendingPathComponent(".cargo/bin/\(name)")
        if files.fileExists(atPath: cargo.path) && cargo != binary { paths.append(cargo) }
        for destination in paths {
            try! Data(contentsOf: root.appendingPathComponent("target/release/\(name)")).write(to: destination, options: .atomic)
            try! files.setAttributes([.posixPermissions: 0o755], ofItemAtPath: destination.path)
            replaced.insert(destination.path)
        }
    }
    reregisterAgents(in: agents, running: replaced)
    let shortcut = binaries.appendingPathComponent("hb")
    if !files.fileExists(atPath: shortcut.path) && (try? files.destinationOfSymbolicLink(atPath: shortcut.path)) == nil {
        try! files.createSymbolicLink(atPath: shortcut.path, withDestinationPath: "hey-boss")
    }
    run(binary.path, ["configure-agents", "--binary", binary.path])
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
