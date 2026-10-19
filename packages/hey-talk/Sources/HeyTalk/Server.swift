import Foundation
import Darwin
import TalkCore

final class CommandServer {
    private var descriptor: Int32 = -1
    private var lockFile: Int32 = -1
    private var source: DispatchSourceRead?
    private let acceptQueue = DispatchQueue(label: "hey-talk.control")
    private let connections = DispatchQueue(label: "hey-talk.connections", attributes: .concurrent)
    private let slots = DispatchSemaphore(value: 16)
    var handle: (([String: Any], @escaping ([String: Any]) -> Void) -> Void)?
    func start() throws {
        let files = FileManager.default
        try files.createDirectory(at: ControlSocket.directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        var info = stat()
        guard lstat(ControlSocket.directory.path, &info) == 0, info.st_uid == getuid(), info.st_mode & S_IFMT == S_IFDIR else {
            throw TalkError("Unsafe hey-talk state directory")
        }
        chmod(ControlSocket.directory.path, 0o700)
        lockFile = open(ControlSocket.directory.appendingPathComponent("service.lock").path, O_CREAT | O_RDWR | O_NOFOLLOW | O_CLOEXEC, 0o600)
        guard lockFile >= 0, flock(lockFile, LOCK_EX | LOCK_NB) == 0 else { throw TalkError("hey-talk is already running") }
        unlink(ControlSocket.path)
        descriptor = socket(AF_UNIX, SOCK_STREAM, 0)
        guard descriptor >= 0 else { throw TalkError("Cannot create command socket") }
        _ = fcntl(descriptor, F_SETFD, FD_CLOEXEC)
        var addr = try ControlSocket.address(ControlSocket.path)
        let bound = withUnsafePointer(to: &addr) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(descriptor, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) } }
        guard bound == 0, chmod(ControlSocket.path, 0o600) == 0, listen(descriptor, 16) == 0 else { throw TalkError("Cannot bind command socket") }
        _ = fcntl(descriptor, F_SETFL, O_NONBLOCK)
        let listener = DispatchSource.makeReadSource(fileDescriptor: descriptor, queue: acceptQueue)
        listener.setEventHandler { [weak self] in self?.acceptClients() }
        listener.resume(); source = listener
    }
    private func acceptClients() {
        while true {
            let fd = accept(descriptor, nil, nil)
            if fd < 0 { return }
            _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
            var uid: uid_t = 0, gid: gid_t = 0
            guard getpeereid(fd, &uid, &gid) == 0, uid == getuid(), slots.wait(timeout: .now()) == .success else { close(fd); continue }
            connections.async { [weak self] in
                guard let self else { close(fd); return }
                defer { self.slots.signal() }
                do {
                    let data = try Wire.line(from: fd, timeout: 5, limit: 16_384)
                    guard let request = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw TalkError("Invalid command") }
                    let finished = DispatchSemaphore(value: 0)
                    let replyLock = NSLock()
                    var replied = false
                    let reply: ([String: Any]) -> Void = { response in
                        replyLock.lock(); defer { replyLock.unlock() }
                        guard !replied else { return }; replied = true
                        try? Wire.write(Wire.json(response), to: fd, timeout: 2)
                        close(fd); finished.signal()
                    }
                    DispatchQueue.main.async { self.handle?(request, reply) }
                    if finished.wait(timeout: .now() + 180) == .timedOut { reply(["ok": false, "error": "Command timed out"]) }
                } catch {
                    try? Wire.write(Wire.json(["ok": false, "error": error.localizedDescription]), to: fd, timeout: 1)
                    close(fd)
                }
            }
        }
    }
    deinit {
        source?.cancel()
        if descriptor >= 0 { close(descriptor); unlink(ControlSocket.path) }
        if lockFile >= 0 { close(lockFile) }
    }
}
