import XCTest
@testable import HeyTalk

private final class FakeSpeechEngine: SpeechEngine {
    var requests: [String] = []
    var replies: [(Result<SpeechAudio, Error>) -> Void] = []
    var cancellations = 0
    var timings: [([SpeechWord]) -> Void] = []
    func warm() {}
    func cancel() { cancellations += 1 }
    func generate(_ text: String, timing: @escaping ([SpeechWord]) -> Void, completion: @escaping (Result<SpeechAudio, Error>) -> Void) { requests.append(text); timings.append(timing); replies.append(completion) }
}
private final class FakeSpeechOutput: SpeechOutput {
    var completions: [() -> Void] = []
    var plays = 0, pauses = 0, stops = 0
    var rate: Float = 1
    var time: Double = 0
    func enqueue(_ audio: SpeechAudio, completion: @escaping () -> Void) throws -> Double { completions.append(completion); return 0 }
    func play() { plays += 1 }
    func pause() { pauses += 1 }
    func stop() { stops += 1 }
}
final class SpeechPlaybackTests: XCTestCase {
    @MainActor func testDefaultSpeakingSpeedIsFasterAndUserCanStillChangeIt() {
        let playback = SpeechPlayback(engine: FakeSpeechEngine(), output: FakeSpeechOutput())
        XCTAssertEqual(playback.rate, 1.25)
        playback.rate = 1.5
        playback.stop()
        XCTAssertEqual(playback.rate, 1.5)
    }
    @MainActor func testAudioClockIsSafeBeforeFirstBuffer() {
        let output = NativeSpeechOutput()
        XCTAssertEqual(output.time, 0)
        output.pause()
        XCTAssertEqual(output.time, 0)
        output.stop()
        XCTAssertEqual(output.time, 0)
    }
    @MainActor func testAudioStartsBeforeTimingsAndHighlightTracksAudioClock() throws {
        let engine = FakeSpeechEngine(), output = FakeSpeechOutput()
        let playback = SpeechPlayback(engine: engine, output: output)
        try playback.start("Hello world.")
        engine.replies[0](.success(SpeechAudio(samples: Array(repeating: 0.1, count: 24_000), sampleRate: 24_000)))
        drain()
        XCTAssertEqual(output.plays, 1, "Playback must not wait for word alignment")
        output.time = 0.3
        XCTAssertEqual(playback.highlight?.word, false)
        engine.timings[0]([SpeechWord(range: NSRange(location: 0, length: 5), start: 0, end: 0.5), SpeechWord(range: NSRange(location: 6, length: 5), start: 0.5, end: 1)])
        drain()
        XCTAssertEqual(playback.highlight?.range, NSRange(location: 0, length: 5))
        output.time = 0.6
        XCTAssertEqual(playback.highlight?.range, NSRange(location: 6, length: 5))
        playback.togglePause()
        XCTAssertEqual(playback.highlight?.range, NSRange(location: 6, length: 5))
        playback.stop()
        XCTAssertNil(playback.highlight)
    }
    @MainActor func testPlaybackStartsBeforeWholePassageAndOnlyBuffersTwoChunks() throws {
        let engine = FakeSpeechEngine(), output = FakeSpeechOutput()
        let playback = SpeechPlayback(engine: engine, output: output)
        try playback.start("First sentence. Second sentence. Third sentence.")
        XCTAssertEqual(engine.requests, ["First sentence."])
        engine.replies[0](.success(SpeechAudio(samples: [0.1], sampleRate: 24000)))
        drain()
        XCTAssertEqual(playback.state, .playing)
        XCTAssertEqual(output.plays, 1)
        XCTAssertEqual(engine.requests.count, 2)
        engine.replies[1](.success(SpeechAudio(samples: [0.1], sampleRate: 24000)))
        drain()
        XCTAssertEqual(engine.requests.count, 2)
        output.completions[0]()
        drain()
        XCTAssertEqual(engine.requests.count, 3)
        playback.stop()
    }
    @MainActor func testPauseDuringLoadingAndCancellationDiscardLateAudio() throws {
        let engine = FakeSpeechEngine(), output = FakeSpeechOutput()
        let playback = SpeechPlayback(engine: engine, output: output)
        try playback.start("Hello there.")
        playback.togglePause()
        engine.replies[0](.success(SpeechAudio(samples: [0.1], sampleRate: 24000)))
        drain()
        XCTAssertEqual(playback.state, .paused)
        XCTAssertEqual(output.plays, 0)
        playback.togglePause()
        XCTAssertEqual(output.plays, 1)
        playback.stop()
        output.completions[0]()
        drain()
        XCTAssertEqual(playback.state, .idle)
        try playback.start("Another sentence.")
        playback.stop()
        engine.replies[1](.success(SpeechAudio(samples: [0.1], sampleRate: 24000)))
        drain()
        XCTAssertEqual(output.completions.count, 1)
        XCTAssertEqual(playback.state, .idle)
    }
    @MainActor func testFailureStopsOutputAndAllowsRetry() throws {
        let engine = FakeSpeechEngine(), output = FakeSpeechOutput()
        let playback = SpeechPlayback(engine: engine, output: output)
        try playback.start("Hello there.")
        engine.replies[0](.failure(NSError(domain: "test", code: 1)))
        drain()
        XCTAssertEqual(playback.state, .failed)
        XCTAssertFalse(playback.active)
        XCTAssertNotNil(playback.error)
        try playback.start("Try again.")
        XCTAssertEqual(playback.state, .loading)
        XCTAssertNil(playback.error)
        playback.stop()
    }
    @MainActor private func drain() {
        let done = expectation(description: "main callbacks")
        DispatchQueue.main.async { done.fulfill() }
        wait(for: [done], timeout: 1)
    }
}
