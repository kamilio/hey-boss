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

    func testEmptyAttemptPreservesLastTranscriptAndAllowsRecordingAgain() throws {
        var session = RecordingSession()
        let spoken = try session.begin()
        try session.processing(spoken)
        XCTAssertTrue(session.finish(spoken, text: "Keep this transcript"))

        for _ in 0..<3 {
            let silent = try session.begin()
            try session.processing(silent)
            XCTAssertTrue(session.finish(silent, text: ""))
            XCTAssertEqual(session.phase, .idle)
            XCTAssertEqual(session.lastText, "Keep this transcript")
        }

        let next = try session.begin()
        XCTAssertFalse(session.finish(spoken, text: ""))
        XCTAssertEqual(session.phase, .recording)
        try session.processing(next)
        XCTAssertTrue(session.finish(next, text: "Next utterance"))
        XCTAssertEqual(session.lastText, "Next utterance")
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

    func testBothShortcutsToggleOncePerPressWithTheirDeliveryMode() {
        var hotkey = HotkeyState()
        XCTAssertEqual(DictationShortcut.paste.keyCode, 14)
        XCTAssertEqual(DictationShortcut.submit.keyCode, 15)
        XCTAssertEqual(hotkey.keyDown(code: 12, hyper: true, repeated: false), .none)
        XCTAssertEqual(hotkey.keyDown(code: 15, hyper: false, repeated: false), .none)
        for shortcut in DictationShortcut.allCases {
            for _ in 0..<100 {
                XCTAssertEqual(hotkey.keyDown(code: shortcut.keyCode, hyper: true, repeated: false), .toggle(shortcut))
                XCTAssertEqual(hotkey.keyDown(code: shortcut.keyCode, hyper: true, repeated: false), .none)
                XCTAssertEqual(hotkey.keyDown(code: shortcut.keyCode, hyper: true, repeated: true), .none)
                XCTAssertEqual(hotkey.keyUp(code: shortcut.keyCode), .none)
            }
        }
    }

    func testReleasingOtherKeyDoesNotRetriggerHeldShortcut() {
        var hotkey = HotkeyState()
        XCTAssertEqual(hotkey.keyDown(code: 14, hyper: true, repeated: false), .toggle(.paste))
        _ = hotkey.keyUp(code: 15)
        XCTAssertEqual(hotkey.keyDown(code: 14, hyper: true, repeated: false), .none)
        XCTAssertEqual(hotkey.keyDown(code: 15, hyper: true, repeated: false), .none)
        _ = hotkey.keyUp(code: 14)
        XCTAssertEqual(hotkey.keyDown(code: 15, hyper: true, repeated: false), .toggle(.submit))
    }

    func testHyperVTogglesAutoOnceWithoutChangingExistingShortcuts() {
        var hotkey = HotkeyState()
        XCTAssertEqual(hotkey.keyDown(code: 9, hyper: false, repeated: false), .none)
        XCTAssertEqual(hotkey.keyDown(code: 9, hyper: true, repeated: false), .toggleAuto)
        XCTAssertEqual(hotkey.keyDown(code: 9, hyper: true, repeated: true), .none)
        XCTAssertEqual(hotkey.keyDown(code: 14, hyper: true, repeated: false), .none)
        _ = hotkey.keyUp(code: 9)
        XCTAssertEqual(hotkey.keyDown(code: 14, hyper: true, repeated: false), .toggle(.paste))
        _ = hotkey.keyUp(code: 14)
        XCTAssertEqual(hotkey.keyDown(code: 15, hyper: true, repeated: false), .toggle(.submit))
    }

    func testEscCancelsWithoutRetriggeringHeldShortcut() {
        var hotkey = HotkeyState()
        _ = hotkey.keyDown(code: 15, hyper: true, repeated: false)
        XCTAssertEqual(hotkey.keyDown(code: 53, hyper: false, repeated: false), .cancel)
        XCTAssertEqual(hotkey.keyDown(code: 15, hyper: true, repeated: false), .none)
        XCTAssertEqual(hotkey.keyUp(code: 15), .none)
        XCTAssertEqual(hotkey.keyDown(code: 15, hyper: true, repeated: false), .toggle(.submit))
    }
}
