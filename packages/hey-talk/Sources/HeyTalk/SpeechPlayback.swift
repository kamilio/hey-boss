import AppKit
import AVFoundation
import TalkCore

protocol SpeechOutput: AnyObject {
    var rate: Float { get set }
    var time: Double { get }
    func enqueue(_ audio: SpeechAudio, completion: @escaping () -> Void) throws -> Double
    func play()
    func pause()
    func stop()
}

final class NativeSpeechOutput: SpeechOutput {
    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private let speed = AVAudioUnitTimePitch()
    private var configured = false
    private var scheduledFrames: AVAudioFramePosition = 0
    private var pausedTime: Double?
    var onInterruption: (() -> Void)?
    private var observer: NSObjectProtocol?
    var rate: Float { get { speed.rate } set { speed.rate = newValue } }
    var time: Double {
        if let pausedTime { return pausedTime }
        guard configured, let render = player.lastRenderTime, let position = player.playerTime(forNodeTime: render) else { return 0 }
        return max(0, Double(position.sampleTime) / position.sampleRate - engine.outputNode.presentationLatency * Double(rate))
    }
    init() {
        observer = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main) { [weak self] _ in
            self?.onInterruption?()
        }
    }
    func enqueue(_ audio: SpeechAudio, completion: @escaping () -> Void) throws -> Double {
        guard let format = AVAudioFormat(standardFormatWithSampleRate: audio.sampleRate, channels: 1),
              !audio.samples.isEmpty, audio.samples.count <= 48_000 * 120,
              let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(audio.samples.count)),
              let samples = buffer.floatChannelData?[0] else { throw TalkError("Cannot prepare speech audio") }
        buffer.frameLength = buffer.frameCapacity
        audio.samples.withUnsafeBufferPointer { samples.update(from: $0.baseAddress!, count: $0.count) }
        if !configured {
            engine.attach(player); engine.attach(speed)
            engine.connect(player, to: speed, format: format)
            engine.connect(speed, to: engine.mainMixerNode, format: format)
            configured = true
        }
        if !engine.isRunning { try engine.start() }
        let rendered: AVAudioFramePosition
        if let render = player.lastRenderTime, let position = player.playerTime(forNodeTime: render) { rendered = position.sampleTime }
        else { rendered = 0 }
        let start = max(scheduledFrames, rendered)
        scheduledFrames = start + AVAudioFramePosition(buffer.frameLength)
        player.scheduleBuffer(buffer, at: AVAudioTime(sampleTime: start, atRate: audio.sampleRate), completionCallbackType: .dataPlayedBack) { _ in DispatchQueue.main.async(execute: completion) }
        return Double(start) / audio.sampleRate
    }
    func play() { guard configured else { return }; pausedTime = nil; player.play() }
    func pause() { pausedTime = time; if configured { player.pause() } }
    func stop() { if configured { player.stop(); engine.stop() }; scheduledFrames = 0; pausedTime = nil }
    deinit { if let observer { NotificationCenter.default.removeObserver(observer) } }
}

enum SpeechState: String { case idle, loading, playing, paused, failed }

// In an unaligned phrase, the drawer highlights the phrase while a one-line
// caption follows the estimated reading position within it.
struct SpeechHighlight {
    var range: NSRange
    let word: Bool
    var follow: Int? = nil
}

// Main-thread state machine. Generation tickets discard worker/player callbacks
// after stop, app switches, or replacement. Generation continues only two chunks ahead.
final class SpeechPlayback {
    private let engine: SpeechEngine
    private let output: SpeechOutput
    private var chunks: [SpeechSegment] = []
    private struct TimedSegment { let start: Double; let duration: Double; var words: [SpeechWord] }
    private var timings: [Int: TimedSegment] = [:]
    private(set) var text = ""
    private var next = 0
    private var queued = 0
    private var streaming = false
    private var generating = false
    private var generation: UInt64 = 0
    private var started: TimeInterval = 0
    private(set) var state: SpeechState = .idle
    private(set) var error: String?
    private(set) var firstAudioMS: Double?
    private(set) var completed = 0
    var onChange: (() -> Void)?
    var active: Bool { [.loading, .playing, .paused].contains(state) }
    var rate: Float { get { output.rate } set { output.rate = min(2, max(0.75, newValue)); onChange?() } }
    var progress: Double { chunks.isEmpty ? 0 : Double(completed) / Double(chunks.count) }
    var timeline: (fraction: Double, remaining: Double)? {
        guard active, !streaming, !generating, next == chunks.count, let last = timings[next - 1] else { return nil }
        let end = last.start + last.duration
        let played = min(output.time, end)
        return (played / end, (end - played) / Double(rate))
    }
    var highlight: SpeechHighlight? {
        guard active else { return nil }
        let time = output.time
        for index in timings.keys.sorted() {
            guard let timing = timings[index], time >= timing.start, time < timing.start + timing.duration else { continue }
            let position = time - timing.start
            if let word = timing.words.first(where: { position >= $0.start && position < $0.end }) {
                return SpeechHighlight(range: NSRange(location: chunks[index].range.location + word.range.location, length: word.range.length), word: true)
            }
            let chunk = chunks[index]
            let before = timing.words.last { $0.end <= position }
            let after = timing.words.first { $0.start > position }
            let start = before?.end ?? 0, end = after?.start ?? timing.duration
            let lower = before.map { NSMaxRange($0.range) } ?? 0
            let upper = after?.range.location ?? chunk.range.length
            let fraction = (position - start) / (end - start)
            let offset = min(chunk.range.length - 1, lower + Int(Double(max(0, upper - lower - 1)) * fraction))
            let anchor = (chunk.text as NSString).rangeOfComposedCharacterSequence(at: offset).location
            return SpeechHighlight(range: chunk.range, word: false, follow: chunk.range.location + anchor)
        }
        return nil
    }
    init(engine: SpeechEngine, output: SpeechOutput) { self.engine = engine; self.output = output; output.rate = 1.25 }
    func start(_ text: String, streaming: Bool = false) throws {
        let parts = try SpeechText.segments(text)
        if active { stop() }
        generation &+= 1
        output.stop()
        self.streaming = streaming
        self.text = text; chunks = parts; timings = [:]; next = 0; queued = 0; completed = 0; generating = false
        error = nil; firstAudioMS = nil; started = ProcessInfo.processInfo.systemUptime
        state = .loading; onChange?(); pump()
    }
    func append(_ sentence: String) throws {
        guard streaming, active else { return }
        let offset = text.utf16.count
        let parts = try SpeechText.segments(sentence)
        text += sentence
        chunks += parts.map { SpeechSegment(text: $0.text, range: NSRange(location: offset + $0.range.location, length: $0.range.length)) }
        onChange?(); pump()
    }
    func finishStream() {
        guard streaming else { return }
        streaming = false
        if completed == chunks.count { output.stop(); state = .idle }
        onChange?()
    }
    func togglePause() {
        guard active else { return }
        if state == .paused {
            state = queued > 0 ? .playing : .loading
            if queued > 0 { output.play(); markFirstAudio() }
        } else { output.pause(); state = .paused }
        onChange?()
    }
    func stop() {
        generation &+= 1
        output.stop(); engine.cancel()
        streaming = false; chunks = []; timings = [:]; next = 0; queued = 0; generating = false; completed = 0
        state = .idle; onChange?()
    }
    func fail(_ message: String) {
        stop(); error = message; state = .failed; onChange?()
    }
    private func markFirstAudio() {
        if firstAudioMS == nil { firstAudioMS = (ProcessInfo.processInfo.systemUptime - started) * 1000 }
    }
    private func pump() {
        guard active, !generating, next - completed < 2, next < chunks.count else { return }
        generating = true
        let ticket = generation, index = next, text = chunks[next].text
        next += 1
        engine.generate(text, timing: { [weak self] words in DispatchQueue.main.async {
            guard let self, self.generation == ticket, self.active else { return }
            self.timings[index]?.words = words
        } }) { [weak self] result in DispatchQueue.main.async {
            guard let self, self.generation == ticket, self.active else { return }
            switch result {
            case .failure(let error): self.fail(error.localizedDescription)
            case .success(let audio):
                self.generating = !audio.final
                do {
                    let start = try self.output.enqueue(audio) { [weak self] in
                        guard let self, self.generation == ticket, self.active else { return }
                        self.queued -= 1; if audio.final { self.completed += 1 }
                        if !self.streaming && self.completed == self.chunks.count {
                            self.output.stop(); self.state = .idle; self.onChange?()
                        } else { self.onChange?(); self.pump() }
                    }
                    let beginning = self.timings[index]?.start ?? start
                    let duration = start - beginning + Double(audio.samples.count) / audio.sampleRate
                    let estimate = self.chunks[index].estimatedSeconds
                    self.timings[index] = TimedSegment(start: beginning, duration: audio.final ? duration : max(duration, estimate), words: [])
                    self.queued += 1
                    if self.state != .paused {
                        if self.state == .loading { self.output.play(); self.markFirstAudio() }
                        self.state = .playing
                    }
                    self.onChange?(); self.pump()
                } catch { self.fail(error.localizedDescription) }
            }
        } }
    }
}
