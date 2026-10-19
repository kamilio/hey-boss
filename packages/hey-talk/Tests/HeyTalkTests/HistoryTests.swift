import XCTest
import TalkCore
@testable import HeyTalk

final class HistoryControllerTests: XCTestCase {
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
