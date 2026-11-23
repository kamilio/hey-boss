import Foundation
import Darwin

// A user-only Unix socket keeps CLI mutations serialized by the running app.
let socketPath = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Sticky/control.sock").path

private func address(_ path: String) throws -> sockaddr_un {
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let capacity = MemoryLayout.size(ofValue: address.sun_path)
    guard path.utf8.count < capacity else { throw StickyError("Sticky's socket path is too long.") }
    withUnsafeMutablePointer(to: &address.sun_path) { pointer in
        pointer.withMemoryRebound(to: CChar.self, capacity: capacity) { output in
            path.withCString { input in _ = strcpy(output, input) }
        }
    }
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
    return address
}
private func withAddress<T>(_ address: inout sockaddr_un, _ body: (UnsafePointer<sockaddr>, socklen_t) -> T) -> T {
    withUnsafePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { body($0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
    }
}
private func prepare(_ fd: Int32) {
    var enabled: Int32 = 1
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &enabled, socklen_t(MemoryLayout.size(ofValue: enabled)))
    var timeout = timeval(tv_sec: 10, tv_usec: 0)
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout.size(ofValue: timeout)))
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout.size(ofValue: timeout)))
}
private func sendJSON(_ value: [String: Any], to fd: Int32) throws {
    var data = try JSONSerialization.data(withJSONObject: value)
    data.append(10)
    try data.withUnsafeBytes { bytes in
        var sent = 0
        while sent < bytes.count {
            let count = Darwin.write(fd, bytes.baseAddress!.advanced(by: sent), bytes.count - sent)
            guard count > 0 else { throw StickyError("Could not send a command to Sticky.") }
            sent += count
        }
    }
}
private func readJSON(_ fd: Int32) throws -> [String: Any] {
    var data = Data(), buffer = [UInt8](repeating: 0, count: 16384)
    while data.count <= 8_388_608 {
        let count = Darwin.read(fd, &buffer, buffer.count)
        guard count > 0 else { throw StickyError("Sticky did not finish its response.") }
        data.append(contentsOf: buffer.prefix(count))
        if data.last == 10 {
            guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { break }
            return object
        }
    }
    throw StickyError("Invalid or oversized Sticky command.")
}
func request(_ command: [String: Any]) throws -> [String: Any] {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { throw StickyError("Could not create a socket.") }
    defer { close(fd) }
    prepare(fd)
    var addr = try address(socketPath)
    guard withAddress(&addr, { connect(fd, $0, $1) }) == 0 else { throw StickyError("Sticky is not running.") }
    try sendJSON(command, to: fd)
    let response = try readJSON(fd)
    if let error = response["error"] as? String { throw StickyError(error) }
    return response
}

final class CommandServer {
    private var fd: Int32 = -1
    private var source: DispatchSourceRead?
    private var lock: Int32 = -1
    init(handler: @escaping ([String: Any]) -> [String: Any]) throws {
        let directory = URL(fileURLWithPath: socketPath).deletingLastPathComponent()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        lock = Darwin.open(directory.appendingPathComponent("app.lock").path, O_CREAT | O_RDWR, 0o600)
        guard lock >= 0, flock(lock, LOCK_EX | LOCK_NB) == 0 else { throw StickyError("Sticky is already running.") }
        unlink(socketPath)
        fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw StickyError("Could not create Sticky's socket.") }
        var addr = try address(socketPath)
        guard withAddress(&addr, { bind(fd, $0, $1) }) == 0, chmod(socketPath, 0o600) == 0, listen(fd, 16) == 0 else {
            throw StickyError("Could not listen on Sticky's socket.")
        }
        let queue = DispatchQueue(label: "sticky.commands")
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: queue)
        source.setEventHandler { [weak self] in
            guard let self else { return }
            let client = accept(self.fd, nil, nil)
            guard client >= 0 else { return }
            defer { close(client) }
            prepare(client)
            do {
                let command = try readJSON(client)
                let result = DispatchQueue.main.sync { handler(command) }
                try sendJSON(result, to: client)
            } catch { try? sendJSON(["error": error.localizedDescription], to: client) }
        }
        self.source = source
        source.resume()
    }
    deinit {
        source?.cancel()
        if fd >= 0 { close(fd); unlink(socketPath) }
        if lock >= 0 { close(lock) }
    }
}
