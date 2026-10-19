import XCTest
@testable import TalkCore

final class HistoryTests: XCTestCase {
    func testPagingWhileNewDictationsArriveDoesNotSkipOrDuplicate() throws {
        let directory = temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = try HistoryStore(directory: directory)
        let entries = (0..<125).map { Dictation(text: "Dictation \($0)", audioSeconds: 1, source: "microphone") }
        for entry in entries { try store.append(entry) }
        let first = try store.recent()
        try store.append(Dictation(text: "Arrived during paging", audioSeconds: 1, source: "microphone"))
        let second = try store.recent(before: first.last!.id)
        let third = try store.recent(before: second.last!.id)
        XCTAssertEqual((first + second + third).map(\.id), entries.reversed().map(\.id))
        XCTAssertEqual(try store.recent().first?.text, "Arrived during paging")
    }

    private func temporaryDirectory() -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    }

    func testHistorySurvivesRestartAndPagesNewestFirst() throws {
        let directory = temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let first = Dictation(text: "First — 你好", audioSeconds: 1, source: "microphone")
        let second = Dictation(text: "Second\nline", audioSeconds: 2, source: "file")
        do {
            let store = try HistoryStore(directory: directory)
            try store.append(first)
            try store.append(second)
        }
        let reopened = try HistoryStore(directory: directory)
        XCTAssertEqual(try reopened.recent(limit: 1), [second])
        XCTAssertEqual(try reopened.recent(limit: 1, before: second.id), [first])
        XCTAssertEqual(try reopened.recent(limit: 10, before: first.id), [])
        let mode = try FileManager.default.attributesOfItem(atPath: directory.appendingPathComponent("history.sqlite3").path)[.posixPermissions] as? Int
        XCTAssertEqual(mode, 0o600)
    }

    func testFailedWritePreservesPreviousTranscript() throws {
        let directory = temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = try HistoryStore(directory: directory)
        let entry = Dictation(text: "Keep this", audioSeconds: 2, source: "microphone")
        try store.append(entry)
        XCTAssertThrowsError(try store.append(entry))
        XCTAssertEqual(try store.recent(), [entry])
    }

    func testCorruptHistoryIsReportedAndNeverOverwritten() throws {
        let directory = temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let file = directory.appendingPathComponent("history.sqlite3")
        let bytes = Data("existing damaged history".utf8)
        try bytes.write(to: file)
        XCTAssertThrowsError(try HistoryStore(directory: directory))
        XCTAssertEqual(try Data(contentsOf: file), bytes)
    }
}
