import Foundation
import Darwin

func setupHeyBoss(_ statePath: String, _ binaryPath: String, _ agentsPath: String, _ daemonPath: String) {
    precondition(ProcessInfo.processInfo.operatingSystemVersion.majorVersion >= 26)
    let state = URL(fileURLWithPath: statePath)
    let binaries = URL(fileURLWithPath: binaryPath)
    let agents = URL(fileURLWithPath: agentsPath)
    let files = FileManager.default
    for directory in [state, agents] {
        try! files.createDirectory(at: directory, withIntermediateDirectories: true)
    }
    try! files.setAttributes([.posixPermissions: 0o700], ofItemAtPath: state.path)
    let config = binaries.appendingPathComponent("hey-boss.state")
    try! Data(state.path.utf8).write(to: config, options: .atomic)
    try! files.setAttributes([.posixPermissions: 0o600], ofItemAtPath: config.path)
    let domain = "gui/\(getuid())"
    let plist = agents.appendingPathComponent("local.hey-boss.plist")
    func launchctl(_ arguments: [String]) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        process.arguments = arguments
        try! process.run()
        process.waitUntilExit()
        precondition(process.terminationStatus == 0)
    }
    if files.fileExists(atPath: plist.path) { launchctl(["bootout", domain, plist.path]) }
    let content: [String: Any] = [
        "Label": "local.hey-boss",
        "ProgramArguments": [daemonPath],
        "EnvironmentVariables": ["HEY_BOSS_STATE_DIR": state.path],
        "LimitLoadToSessionType": "Aqua",
        "Sockets": ["Listener": ["SockPathName": state.appendingPathComponent("daemon.sock").path, "SockPathMode": 0o600, "SockType": "stream"]],
        "StandardOutPath": state.appendingPathComponent("daemon.log").path,
        "StandardErrorPath": state.appendingPathComponent("daemon.log").path,
        "ProcessType": "Interactive"
    ]
    try! PropertyListSerialization.data(fromPropertyList: content, format: .xml, options: 0).write(to: plist, options: .atomic)
    try! files.setAttributes([.posixPermissions: 0o600], ofItemAtPath: plist.path)
    launchctl(["bootstrap", domain, plist.path])
}

precondition(CommandLine.arguments.count == 5)
setupHeyBoss(CommandLine.arguments[1], CommandLine.arguments[2], CommandLine.arguments[3], CommandLine.arguments[4])
