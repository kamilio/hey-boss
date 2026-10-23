import XCTest
import SQLite3
@testable import TalkCore

final class HistoryTests: XCTestCase {
    func testOriginalSurvivesCorrectionAndRestartExactly() throws {
        let directory = temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let original = "um  deploy café's API\nuh tomorrow?"
        let entry = Dictation(text: "Deploy café's API tomorrow?", originalText: original, audioSeconds: 3, source: "microphone")
        do { try HistoryStore(directory: directory).append(entry) }
        let restored = try XCTUnwrap(HistoryStore(directory: directory).recent().first)
        XCTAssertEqual(restored, entry)
        XCTAssertEqual(restored.originalText, original)
        XCTAssertEqual(restored.json["original_text"] as? String, original)
        XCTAssertEqual(restored.json["text"] as? String, entry.text)
    }

    func testLegacyHistoryMigratesWithoutInventingOriginals() throws {
        let directory = temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        var database: OpaquePointer?
        XCTAssertEqual(sqlite3_open(directory.appendingPathComponent("history.sqlite3").path, &database), SQLITE_OK)
        let id = UUID()
        XCTAssertEqual(sqlite3_exec(database, "CREATE TABLE dictations (id TEXT NOT NULL UNIQUE, created_at REAL NOT NULL, text TEXT NOT NULL, audio_seconds REAL NOT NULL, source TEXT NOT NULL)", nil, nil, nil), SQLITE_OK)
        XCTAssertEqual(sqlite3_exec(database, "INSERT INTO dictations VALUES ('\(id.uuidString)', 0, 'Old corrected text', 2, 'microphone')", nil, nil, nil), SQLITE_OK)
        sqlite3_close(database)
        do {
            let store = try HistoryStore(directory: directory)
            let legacy = try XCTUnwrap(store.recent().first)
            XCTAssertEqual(legacy.id, id)
            XCTAssertEqual(legacy.text, "Old corrected text")
            XCTAssertNil(legacy.originalText)
            XCTAssertTrue(legacy.json["original_text"] is NSNull)
            try store.append(Dictation(text: "Unchanged", originalText: "Unchanged", audioSeconds: 1, source: "file"))
        }
        let entries = try HistoryStore(directory: directory).recent()
        XCTAssertEqual(entries.count, 2)
        XCTAssertEqual(entries[0].originalText, "Unchanged")
        XCTAssertNil(entries[1].originalText)
    }

    func testPagingWhileNewDictationsArriveDoesNotSkipOrDuplicate() throws {
        let directory = temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = try HistoryStore(directory: directory)
        let entries = (0..<125).map { Dictation(text: "Dictation \($0)", originalText: "Dictation \($0)", audioSeconds: 1, source: "microphone") }
        for entry in entries { try store.append(entry) }
        let first = try store.recent()
        try store.append(Dictation(text: "Arrived during paging", originalText: "Arrived during paging", audioSeconds: 1, source: "microphone"))
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
        let first = Dictation(text: "First — 你好", originalText: "First — 你好", audioSeconds: 1, source: "microphone")
        let second = Dictation(text: "Second\nline", originalText: "Second\nline", audioSeconds: 2, source: "file")
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
        let entry = Dictation(text: "Keep this", originalText: "Keep this", audioSeconds: 2, source: "microphone")
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
