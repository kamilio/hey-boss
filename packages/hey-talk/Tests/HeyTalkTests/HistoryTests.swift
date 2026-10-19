import XCTest
import TalkCore
import SQLite3
@testable import HeyTalk

final class HistoryControllerTests: XCTestCase {
    func testLockedDatabaseKeepsTranscriptWithoutBlockingUI() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = try HistoryStore(directory: directory)
        let previous = Dictation(text: "Previous", audioSeconds: 1, source: "file")
        try store.append(previous)
        var lock: OpaquePointer?
        XCTAssertEqual(sqlite3_open(directory.appendingPathComponent("history.sqlite3").path, &lock), SQLITE_OK)
        defer { sqlite3_exec(lock, "ROLLBACK", nil, nil, nil); sqlite3_close(lock) }
        XCTAssertEqual(sqlite3_exec(lock, "BEGIN EXCLUSIVE", nil, nil, nil), SQLITE_OK)
        let history = DictationHistory(directory: directory)
        let entry = Dictation(text: "Keep despite locked disk", audioSeconds: 1, source: "microphone")
        let completed = expectation(description: "locked write returns failure")
        let responsive = expectation(description: "main queue stays responsive")
        history.save(entry) { saved in
            XCTAssertFalse(saved)
            XCTAssertEqual(history.entries, [entry])
            XCTAssertTrue(history.unsaved.contains(entry.id))
            completed.fulfill()
        }
        DispatchQueue.main.async { responsive.fulfill() }
        wait(for: [responsive], timeout: 0.5)
        wait(for: [completed], timeout: 4)
        XCTAssertEqual(sqlite3_exec(lock, "ROLLBACK", nil, nil, nil), SQLITE_OK)
        XCTAssertEqual(try store.recent(), [previous])
    }

    func testSaveCompletionRunsAfterDurableCommit() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let history = DictationHistory(directory: directory)
        let entry = Dictation(text: "Saved before paste", audioSeconds: 1, source: "microphone")
        let completed = expectation(description: "saved")
        history.save(entry) { saved in
            XCTAssertTrue(Thread.isMainThread)
            XCTAssertTrue(saved)
            XCTAssertEqual(try? HistoryStore(directory: directory).recent(), [entry])
            XCTAssertEqual(history.entries, [entry])
            completed.fulfill()
        }
        wait(for: [completed], timeout: 5)
    }

    func testDiskFailureRetainsTextForCopyAndReportsUnsaved() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        try Data("not a directory".utf8).write(to: directory)
        let history = DictationHistory(directory: directory)
        let entry = Dictation(text: "Still available to copy", audioSeconds: 1, source: "microphone")
        let completed = expectation(description: "failure handled")
        history.save(entry) { saved in
            XCTAssertFalse(saved)
            XCTAssertEqual(history.entries, [entry])
            XCTAssertTrue(history.unsaved.contains(entry.id))
            XCTAssertNotNil(history.warning)
            completed.fulfill()
        }
        wait(for: [completed], timeout: 5)
    }
}
