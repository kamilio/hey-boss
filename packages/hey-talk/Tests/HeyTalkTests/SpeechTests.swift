import AppKit
import TalkCore
import XCTest
@testable import HeyTalk

final class SpeechTests: XCTestCase {
    func testAllowlistDefaultsToT3AndEmptyListStaysEmpty() throws {
        let suite = "hey-talk-speech-test-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let settings = SpeechSettings(defaults: defaults)
        XCTAssertTrue(settings.enabled)
        XCTAssertEqual(settings.bundleIDs, ["com.t3tools.t3code"])
        XCTAssertFalse(settings.allows("com.apple.Safari"))
        try settings.command(["remove", "com.t3tools.t3code"])
        XCTAssertEqual(SpeechSettings(defaults: defaults).bundleIDs, [])
        try settings.command(["add", "com.example.Reader"])
        try settings.command(["add", "com.example.Reader"])
        XCTAssertEqual(settings.bundleIDs, ["com.example.Reader"])
        try settings.command(["off"])
        XCTAssertFalse(settings.allows("com.example.Reader"))
        for args in [["add"], ["on", "extra"], ["add", "invalid app"], ["remove", ""]] {
            XCTAssertThrowsError(try settings.command(args))
        }
    }

    func testSpeechChunksPreserveWordsAndBoundFirstSoundLatency() throws {
        let text = "Hello there. Here is another sentence! " + String(repeating: "long sentence without punctuation ", count: 40)
        let chunks = try SpeechText.chunks(text)
        XCTAssertEqual(chunks.first, "Hello there.")
        XCTAssertTrue(chunks.allSatisfy { $0.count <= 240 && !$0.isEmpty })
        XCTAssertEqual(chunks.joined(separator: " "), text.trimmingCharacters(in: .whitespacesAndNewlines))
        XCTAssertThrowsError(try SpeechText.chunks("  \n"))
        XCTAssertThrowsError(try SpeechText.chunks(String(repeating: "x", count: 20_001)))
        XCTAssertEqual(try SpeechText.chunks(String(repeating: "🙂", count: 500)).map(\.count), [60, 180, 180, 80])
    }

    func testHyperSDoesNotRepeatAndDoesNotChangeDictationShortcuts() {
        var state = TalkCore.HotkeyState()
        XCTAssertEqual(state.keyDown(code: 1, hyper: true, repeated: false), .toggleSpeech)
        XCTAssertEqual(state.keyDown(code: 1, hyper: true, repeated: true), .none)
        _ = state.keyUp(code: 1)
        XCTAssertEqual(state.keyDown(code: 17, hyper: true, repeated: false), .toggleSpeechMode)
        _ = state.keyUp(code: 17)
        XCTAssertEqual(state.keyDown(code: 1, hyper: false, repeated: false), .none)
        XCTAssertEqual(state.keyDown(code: 14, hyper: true, repeated: false), .toggle(.autoSubmit))
    }
}
