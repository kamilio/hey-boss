import XCTest
@testable import TalkCore

final class CoreTests: XCTestCase {
    func testCancelInvalidatesInFlightWork() throws {
        var session = RecordingSession()
        let first = try session.begin()
        try session.processing(first)
        session.cancel()
        let second = try session.begin()
        XCTAssertFalse(session.finish(first, text: "stale"))
        XCTAssertEqual(session.phase, .recording)
        try session.processing(second)
        XCTAssertTrue(session.finish(second, text: "current"))
        XCTAssertEqual(session.lastText, "current")
    }

    func testStartCannotOverlapRecordingOrInference() throws {
        var session = RecordingSession()
        let id = try session.begin()
        XCTAssertThrowsError(try session.begin())
        try session.processing(id)
        XCTAssertThrowsError(try session.begin())
    }

    func testWaveformMatchesReferenceAndRejectsInvalidLevels() {
        XCTAssertEqual(Waveform.count, 11)
        XCTAssertEqual(Waveform.height(0), 4)
        XCTAssertEqual(Waveform.height(1), 22)
        XCTAssertEqual(Waveform.height(.nan), 4)
        XCTAssertEqual(Waveform.height(-1), 4)
        XCTAssertGreaterThan(Waveform.height(0.02), 4)
        XCTAssertLessThan(Waveform.height(0.02), Waveform.height(0.1))
    }

    func testWorkerFrameRoundTripAndBoundedAudio() throws {
        let samples: [Float] = [0, -0.25, 0.5]
        let frame = try PCMFrame.encode(samples)
        XCTAssertEqual(try PCMFrame.decode(frame), samples)
        XCTAssertThrowsError(try PCMFrame.decode(Data([0, 0])))
        var malformed = frame
        malformed[0] = 10
        XCTAssertThrowsError(try PCMFrame.decode(malformed))
        XCTAssertThrowsError(try PCMFrame.encode([Float.nan]))
        XCTAssertThrowsError(try PCMFrame.encode([]))
    }

    func testHyperReleaseStopsEvenWhenModifiersReleasedFirst() {
        var hotkey = HotkeyState()
        XCTAssertEqual(hotkey.keyDown(code: 14, hyper: true, repeated: false), .start)
        XCTAssertEqual(hotkey.keyDown(code: 14, hyper: true, repeated: true), .none)
        XCTAssertEqual(hotkey.modifiersChanged(hyper: false), .stop)
        XCTAssertEqual(hotkey.keyUp(code: 14), .none)
        XCTAssertEqual(hotkey.keyDown(code: 14, hyper: true, repeated: false), .start)
        XCTAssertEqual(hotkey.keyUp(code: 14), .stop)
    }

    func testEscCancelsAndNeverCausesReleaseToPaste() {
        var hotkey = HotkeyState()
        _ = hotkey.keyDown(code: 14, hyper: true, repeated: false)
        XCTAssertEqual(hotkey.keyDown(code: 53, hyper: false, repeated: false), .cancel)
        XCTAssertEqual(hotkey.keyUp(code: 14), .none)
    }
}
