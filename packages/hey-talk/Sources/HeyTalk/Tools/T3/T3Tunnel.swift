import AppKit

/// T3 Desktop owns one ssh child per open SSH connection. Only consider its
/// loopback forwards; the hostname alone does not identify a T3 environment.
struct T3Tunnel {
    let origin: URL
    let host: String
    let sshPort: UInt16

    init?(arguments: [String]) {
        guard let portFlag = arguments.firstIndex(of: "-p"), portFlag + 1 < arguments.count,
              let sshPort = UInt16(arguments[portFlag + 1]), sshPort > 0,
              Array(arguments.suffix(6).prefix(3)) == ["-n", "-N", "-L"],
              arguments.suffix(2).first == "--", let host = arguments.last else { return nil }
        let forward = arguments[arguments.count - 3].components(separatedBy: ":")
        guard forward.count == 3, forward[1] == "127.0.0.1", UInt16(forward[2]) != nil,
              let port = UInt16(forward[0]), port > 0 else { return nil }
        self.origin = URL(string: "http://127.0.0.1:\(port)")!
        self.host = host
        self.sshPort = sshPort
    }

    static func connected() -> [Self] {
        guard let app = NSRunningApplication.runningApplications(withBundleIdentifier: "com.t3tools.t3code").first else { return [] }
        let pid = app.processIdentifier
        let count = proc_listchildpids(pid, nil, 0)
        guard count > 0 else { return [] }
        var children = [pid_t](repeating: 0, count: Int(count) + 8)
        let capacity = Int32(children.count * MemoryLayout<pid_t>.size)
        let found = proc_listchildpids(pid, &children, capacity)
        return children.prefix(max(0, Int(found))).compactMap { child in
            guard child > 0 else { return nil }
            var path = [CChar](repeating: 0, count: Int(MAXPATHLEN))
            guard proc_pidpath(child, &path, UInt32(path.count)) > 0, String(cString: path) == "/usr/bin/ssh" else { return nil }
            var mib = [CTL_KERN, KERN_PROCARGS2, child], size = 0
            guard sysctl(&mib, 3, nil, &size, nil, 0) == 0 else { return nil }
            var bytes = [UInt8](repeating: 0, count: size)
            guard sysctl(&mib, 3, &bytes, &size, nil, 0) == 0, size > MemoryLayout<Int32>.size else { return nil }
            let argc = bytes.withUnsafeBytes { Int($0.loadUnaligned(as: Int32.self)) }
            let fields = bytes[4..<size].split(separator: 0, maxSplits: argc + 1)
            return Self(arguments: fields.dropFirst().prefix(argc).map { String(decoding: $0, as: UTF8.self) })
        }
    }
}
