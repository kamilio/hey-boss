import XCTest
import Darwin
import TalkCore
@testable import HeyTalk

final class ServerTests: XCTestCase {
    private func connect(_ directory: URL) throws -> Int32 {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        var address = try ControlSocket.address(directory.appendingPathComponent("control.sock").path)
        let status = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard status == 0 else { close(fd); throw TalkError("Test connection failed") }
        return fd
    }
    private func directory() -> URL { URL(fileURLWithPath: "/tmp/ht-\(UUID())") }

    func testStalledReplyDoesNotBlockMainThread() throws {
        let directory = directory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let server = CommandServer(directory: directory)
        try server.start()
        let responsive = expectation(description: "UI queue runs during blocked reply")
        server.handle = { _, reply in
            let start = ProcessInfo.processInfo.systemUptime
            DispatchQueue.main.async {
                XCTAssertLessThan(ProcessInfo.processInfo.systemUptime - start, 0.5)
                responsive.fulfill()
            }
            reply(["ok": true, "text": String(repeating: "x", count: 4_000_000)])
        }
        let fd = try connect(directory)
        defer { close(fd) }
        try Wire.write(Wire.json(["command": "history"]), to: fd)
        wait(for: [responsive], timeout: 5)
        withExtendedLifetime(server) {}
    }

    func testMalformedClientAndSecondServerCannotBreakService() throws {
        let directory = directory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let server = CommandServer(directory: directory)
        try server.start()
        XCTAssertThrowsError(try CommandServer(directory: directory).start())
        let malformed = try connect(directory)
        try Wire.write(Data("not json\n".utf8), to: malformed)
        let error = try JSONSerialization.jsonObject(with: Wire.line(from: malformed)) as! [String: Any]
        XCTAssertEqual(error["ok"] as? Bool, false)
        close(malformed)
        let completed = expectation(description: "valid client succeeds afterward")
        server.handle = { _, reply in reply(["ok": true]) }
        DispatchQueue.global().async {
            do {
                let fd = try self.connect(directory)
                defer { close(fd) }
                try Wire.write(Wire.json(["command": "status"]), to: fd)
                let response = try JSONSerialization.jsonObject(with: Wire.line(from: fd)) as! [String: Any]
                XCTAssertEqual(response["ok"] as? Bool, true)
            } catch { XCTFail(error.localizedDescription) }
            completed.fulfill()
        }
        wait(for: [completed], timeout: 3)
        withExtendedLifetime(server) {}
    }
}
