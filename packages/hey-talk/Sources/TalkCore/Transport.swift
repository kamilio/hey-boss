import Foundation
import Darwin

public enum Wire {
    /// poll() only bounds IO when the descriptor itself cannot block afterward.
    public static func prepare(_ fd: Int32) throws {
        let flags = fcntl(fd, F_GETFL)
        guard flags >= 0, fcntl(fd, F_SETFL, flags | O_NONBLOCK) == 0,
              fcntl(fd, F_SETFD, FD_CLOEXEC) == 0 else { throw TalkError("Cannot configure connection") }
        var one: Int32 = 1
        _ = setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
    }
    public static func write(_ data: Data, to fd: Int32, timeout: TimeInterval = 30) throws {
        let deadline = Date().addingTimeInterval(timeout)
        try data.withUnsafeBytes { raw in
            var offset = 0
            while offset < raw.count {
                try wait(fd, events: Int16(POLLOUT), deadline: deadline)
                let n = Darwin.write(fd, raw.baseAddress!.advanced(by: offset), min(16_384, raw.count - offset))
                if n < 0 && (errno == EINTR || errno == EAGAIN) { continue }
                guard n > 0 else { throw TalkError("Connection closed while writing") }
                offset += n
            }
        }
    }
    public static func readExactly(_ count: Int, from fd: Int32, timeout: TimeInterval = 30) throws -> Data {
        var data = Data(count: count)
        let deadline = Date().addingTimeInterval(timeout)
        try data.withUnsafeMutableBytes { raw in
            var offset = 0
            while offset < count {
                try wait(fd, events: Int16(POLLIN), deadline: deadline)
                let n = Darwin.read(fd, raw.baseAddress!.advanced(by: offset), count - offset)
                if n < 0 && (errno == EINTR || errno == EAGAIN) { continue }
                guard n > 0 else { throw TalkError("Connection closed while reading") }
                offset += n
            }
        }
        return data
    }
    public static func line(from fd: Int32, timeout: TimeInterval = 120, limit: Int = 1_048_576) throws -> Data {
        let deadline = Date().addingTimeInterval(timeout)
        var data = Data()
        while data.count < limit {
            let byte = try readExactly(1, from: fd, timeout: max(0, deadline.timeIntervalSinceNow))
            if byte[0] == 10 { return data }
            data.append(byte)
        }
        throw TalkError("Response exceeds size limit")
    }
    private static func wait(_ fd: Int32, events: Int16, deadline: Date) throws {
        while true {
            let ms = deadline.timeIntervalSinceNow * 1_000
            guard ms > 0 else { throw TalkError("Operation timed out") }
            var descriptor = pollfd(fd: fd, events: events, revents: 0)
            let result = poll(&descriptor, 1, Int32(min(ms, 2_000_000_000)))
            if result < 0 && errno == EINTR { continue }
            guard result > 0 else { throw TalkError("Operation timed out") }
            guard descriptor.revents & (events | Int16(POLLHUP)) != 0 else { throw TalkError("Connection failed") }
            return
        }
    }
    public static func json(_ value: [String: Any]) throws -> Data {
        var data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
        data.append(10)
        return data
    }
}

public enum ControlSocket {
    public static var directory: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/hey-talk")
    }
    public static var path: String { directory.appendingPathComponent("control.sock").path }
    public static func address(_ path: String) throws -> sockaddr_un {
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8CString)
        guard bytes.count <= MemoryLayout.size(ofValue: address.sun_path) else { throw TalkError("Socket path too long") }
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            for (i, b) in bytes.enumerated() { raw[i] = UInt8(bitPattern: b) }
        }
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        return address
    }
    public static func connect() throws -> Int32 {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw TalkError("Cannot create control socket") }
        var addr = try address(path)
        let result = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard result == 0 else { close(fd); throw TalkError("hey-talk is not running") }
        do { try Wire.prepare(fd) } catch { close(fd); throw error }
        return fd
    }
    public static func request(_ request: [String: Any]) throws -> [String: Any] {
        let fd = try connect()
        defer { close(fd) }
        try Wire.write(Wire.json(request), to: fd)
        let data = try Wire.line(from: fd, timeout: 180)
        guard let response = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw TalkError("Invalid service response")
        }
        return response
    }
}
