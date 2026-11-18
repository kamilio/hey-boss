import Foundation
import TalkCore

final class Server {
    static var path: String { History.directory.appendingPathComponent("control.sock").path }
    private var fd: Int32 = -1
    private var lock: Int32 = -1
    private var source: DispatchSourceRead?
    private let clients = DispatchQueue(label: "hey-clipboard.clients", attributes: .concurrent)
    private let slots = DispatchSemaphore(value: 8)
    var handle: (([String: Any], @escaping ([String: Any]) -> Void) -> Void)?

    func start() throws {
        lock = open(History.directory.appendingPathComponent("service.lock").path, O_CREAT | O_RDWR | O_NOFOLLOW | O_CLOEXEC, 0o600)
        guard lock >= 0, flock(lock, LOCK_EX | LOCK_NB) == 0 else { throw TalkError("Hey Clipboard is already running") }
        fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw TalkError("Cannot create control socket") }
        try Wire.prepare(fd)
        unlink(Self.path)
        var addr = try ControlSocket.address(Self.path)
        let bound = withUnsafePointer(to: &addr) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) } }
        guard bound == 0, chmod(Self.path, 0o600) == 0, listen(fd, 8) == 0 else { throw TalkError("Cannot bind control socket") }
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: clients)
        source.setEventHandler { [weak self] in self?.acceptClients() }
        source.resume(); self.source = source
    }
    private func acceptClients() {
        while true {
            let client = accept(fd, nil, nil)
            guard client >= 0 else { return }
            var uid: uid_t = 0, gid: gid_t = 0
            guard getpeereid(client, &uid, &gid) == 0, uid == getuid(), slots.wait(timeout: .now()) == .success else { close(client); continue }
            clients.async {
                defer { self.slots.signal() }
                do {
                    try Wire.prepare(client)
                    let data = try Wire.line(from: client, timeout: 3, limit: 1_100_000)
                    guard let request = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw TalkError("Invalid JSON request") }
                    let finished = DispatchSemaphore(value: 0), replyLock = NSLock()
                    var replied = false
                    let reply: ([String: Any]) -> Void = { response in
                        replyLock.lock()
                        guard !replied else { replyLock.unlock(); return }
                        replied = true; replyLock.unlock()
                        self.clients.async {
                            try? Wire.write(Wire.json(response), to: client, timeout: 3)
                            close(client); finished.signal()
                        }
                    }
                    DispatchQueue.main.async { self.handle?(request, reply) }
                    if finished.wait(timeout: .now() + 10) == .timedOut { reply(["ok": false, "error": "Command timed out"]) }
                } catch {
                    try? Wire.write(Wire.json(["ok": false, "error": error.localizedDescription]), to: client, timeout: 1)
                    close(client)
                }
            }
        }
    }
    static func request(_ value: [String: Any]) throws -> [String: Any] {
        guard FileManager.default.fileExists(atPath: path) else { throw TalkError("Hey Clipboard is not running. Open Hey Clipboard.app.") }
        let fd = try ControlSocket.connect(path: path)
        defer { close(fd) }
        try Wire.write(Wire.json(value), to: fd, timeout: 3)
        let data = try Wire.line(from: fd, timeout: 12, limit: 8_388_608)
        guard let result = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw TalkError("Invalid service response") }
        return result
    }
    deinit {
        source?.cancel()
        if fd >= 0 { close(fd); unlink(Self.path) }
        if lock >= 0 { close(lock) }
    }
}
