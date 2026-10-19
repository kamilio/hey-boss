import XCTest
import Darwin
@testable import TalkCore

final class TransportTests: XCTestCase {
    func testPartialFrameReadsAndWrites() throws {
        var pair: [Int32] = [0, 0]
        XCTAssertEqual(socketpair(AF_UNIX, SOCK_STREAM, 0, &pair), 0)
        defer { close(pair[0]); close(pair[1]) }
        let sender = pair[0], receiver = pair[1]
        let expected = Data(repeating: 42, count: 200_000)
        let done = expectation(description: "writer finishes")
        DispatchQueue.global().async {
            defer { done.fulfill() }
            do { try Wire.write(expected, to: sender) } catch { XCTFail(error.localizedDescription) }
        }
        XCTAssertEqual(try Wire.readExactly(expected.count, from: receiver), expected)
        wait(for: [done], timeout: 2)
    }
    func testStalledAndOversizedClientsAreBounded() throws {
        var pair: [Int32] = [0, 0]
        XCTAssertEqual(socketpair(AF_UNIX, SOCK_STREAM, 0, &pair), 0)
        defer { close(pair[0]); close(pair[1]) }
        XCTAssertThrowsError(try Wire.line(from: pair[1], timeout: 0.01))
        try Wire.write(Data("oversized\n".utf8), to: pair[0])
        XCTAssertThrowsError(try Wire.line(from: pair[1], limit: 4))
    }
    func testClosedPeerFailsInsteadOfHanging() throws {
        var pair: [Int32] = [0, 0]
        XCTAssertEqual(socketpair(AF_UNIX, SOCK_STREAM, 0, &pair), 0)
        close(pair[0]); defer { close(pair[1]) }
        XCTAssertThrowsError(try Wire.readExactly(4, from: pair[1], timeout: 0.1))
    }
    func testLongSocketPathIsRejected() {
        XCTAssertThrowsError(try ControlSocket.address(String(repeating: "a", count: 105)))
    }
}
