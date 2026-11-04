import AVFoundation
import XCTest
@testable import HeyTalk

final class RecordingArchiveTests: XCTestCase {
    func testOnlyLatestFiveCompleteRecordingsCanBeReplayed() throws {
        let files = FileManager.default
        let root = URL(fileURLWithPath: files.currentDirectoryPath).appendingPathComponent(UUID().uuidString)
        let directory = root.appendingPathComponent("recordings")
        defer { try? files.removeItem(at: root) }
        func samples(_ take: Int) -> [Float] { [0, Float(take) / 9, -0.1234567, 0] }
        for take in 1...8 {
            let history = DictationHistory(directory: root)
            history.saveRecording(samples(take))
            history.saveRecording([])
            history.finishPendingWrites()
            let names = try files.contentsOfDirectory(atPath: directory.path)
            XCTAssertEqual(names.count, min(take, 5))
            for i in max(1, take - 4)...take {
                let url = directory.appendingPathComponent("recording-\(i).wav")
                XCTAssertEqual(try AudioCapture.readFile(url.path), samples(i))
                XCTAssertEqual(try files.attributesOfItem(atPath: url.path)[.posixPermissions] as? Int, 0o600)
            }
        }
    }
}
