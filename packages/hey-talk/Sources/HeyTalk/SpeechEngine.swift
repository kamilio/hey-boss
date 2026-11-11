import Foundation
import CSherpa
import TalkCore
import Darwin
import AVFoundation

struct SpeechAudio {
    let samples: [Float]
    let sampleRate: Double
}

// Remote implementations can deliver the same bounded audio chunks. The player
// and UI do not know where generation happens. No remote requests in this build.
protocol SpeechEngine: AnyObject {
    func generate(_ text: String, timing: @escaping ([SpeechWord]) -> Void, completion: @escaping (Result<SpeechAudio, Error>) -> Void)
    func warm()
    func cancel()
}

struct SpeechModel: Decodable {
    let name: String
    let voice: String
    static let selected: SpeechModel? = {
        let contents = ModelPaths.contents
        let url = contents.lastPathComponent == "Contents"
            ? contents.appendingPathComponent("Resources/speech-model.json")
            : Bundle.module.url(forResource: "speech-model", withExtension: "json")
        guard let url, let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(Self.self, from: data)
    }()
    var directory: URL { ControlSocket.directory.appendingPathComponent("models/" + name) }
    var available: Bool {
        ["config.yaml", "model.safetensors", "tokenizer.json", voice + ".safetensors"].allSatisfy {
            FileManager.default.isReadableFile(atPath: directory.appendingPathComponent($0).path)
        } && FileManager.default.isExecutableFile(atPath: ModelPaths.contents.appendingPathComponent("Helpers/pocket-tts").path)
    }
}

final class LocalSpeechEngine: SpeechEngine {
    private let queue = DispatchQueue(label: "hey-talk.speech-model", qos: .userInitiated)
    private let lock = NSLock()
    private var process: Process?
    private var input: Pipe?
    private var output: Pipe?
    private var epoch: UInt64 = 0
    private var warming = false
    var pid: Int32? { lock.lock(); defer { lock.unlock() }; return process?.isRunning == true ? process?.processIdentifier : nil }

    func warm() {
        lock.lock()
        guard !warming, process?.isRunning != true else { lock.unlock(); return }
        warming = true; let ticket = epoch; lock.unlock()
        queue.async {
            do { _ = try self.worker(ticket) } catch { self.cancel(ticket: ticket) }
            self.lock.lock(); self.warming = false; self.lock.unlock()
        }
    }
    func generate(_ text: String, timing: @escaping ([SpeechWord]) -> Void, completion: @escaping (Result<SpeechAudio, Error>) -> Void) {
        lock.lock(); let ticket = epoch; lock.unlock()
        queue.async {
            var delivered = false
            do {
                let (input, output) = try self.worker(ticket)
                try Wire.write(Wire.json(["text": text]), to: input.fileHandleForWriting.fileDescriptor)
                let header = try Wire.line(from: output.fileHandleForReading.fileDescriptor, timeout: 60, limit: 4096)
                let message = try JSONSerialization.jsonObject(with: header) as? [String: Any]
                guard let rate = message?["rate"] as? Int, (8_000...96_000).contains(rate),
                      let count = message?["count"] as? Int, count > 0, count <= rate * 120 else {
                    throw TalkError(message?["error"] as? String ?? "Invalid speech audio")
                }
                let data = try Wire.readExactly(count * 4, from: output.fileHandleForReading.fileDescriptor, timeout: 30)
                let samples: [Float] = data.withUnsafeBytes { bytes in
                    stride(from: 0, to: data.count, by: 4).map { bytes.loadUnaligned(fromByteOffset: $0, as: Float.self) }
                }
                guard samples.allSatisfy(\.isFinite) else { throw TalkError("Invalid speech samples") }
                self.lock.lock(); let valid = ticket == self.epoch; self.lock.unlock()
                guard valid else { throw TalkError("Speech cancelled") }
                delivered = true
                completion(.success(SpeechAudio(samples: samples, sampleRate: Double(rate))))
                // Audio reaches the player before alignment finishes. Timings
                // follow on the same private pipe while playback is already running.
                let alignment = try JSONSerialization.jsonObject(with: Wire.line(from: output.fileHandleForReading.fileDescriptor, timeout: 30, limit: 65_536)) as? [String: Any]
                timing(SpeechTiming.align(text: text, tokens: alignment?["tokens"] as? [String] ?? [],
                    times: alignment?["timestamps"] as? [Double] ?? [], duration: Double(count) / Double(rate)))
            } catch { self.cancel(ticket: ticket); if !delivered { completion(.failure(error)) } }
        }
    }
    private func worker(_ ticket: UInt64) throws -> (Pipe, Pipe) {
        lock.lock()
        guard ticket == epoch else { lock.unlock(); throw TalkError("Speech cancelled") }
        if process?.isRunning == true, let input, let output { lock.unlock(); return (input, output) }
        guard SpeechModel.selected?.available == true else { lock.unlock(); throw TalkError("Speech model missing. Run the Hey Talk installer.") }
        let child = Process(), stdin = Pipe(), stdout = Pipe()
        child.executableURL = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/hey-talk-inference")
        child.arguments = ["--speech-worker"]
        child.standardInput = stdin; child.standardOutput = stdout; child.standardError = FileHandle.standardError
        child.environment = ProcessInfo.processInfo.environment.filter { !$0.key.hasPrefix("DYLD_") }
        do {
            try Wire.prepare(stdin.fileHandleForWriting.fileDescriptor)
            try Wire.prepare(stdout.fileHandleForReading.fileDescriptor)
            try child.run()
        } catch { lock.unlock(); throw error }
        process = child; input = stdin; output = stdout
        lock.unlock()
        let data = try Wire.line(from: stdout.fileHandleForReading.fileDescriptor, timeout: 60, limit: 4096)
        let ready = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard ready?["ready"] as? Bool == true else { throw TalkError(ready?["error"] as? String ?? "Speech model failed to load") }
        lock.lock(); let valid = ticket == epoch; lock.unlock()
        guard valid else { throw TalkError("Speech cancelled") }
        return (stdin, stdout)
    }
    func cancel() { cancel(ticket: nil) }
    private func cancel(ticket: UInt64?) {
        lock.lock()
        if let ticket, ticket != epoch { lock.unlock(); return }
        epoch &+= 1
        let old = process; process = nil; input = nil; output = nil
        lock.unlock()
        guard let old, old.isRunning else { return }
        old.terminate()
        DispatchQueue.global().asyncAfter(deadline: .now() + 1) { if old.isRunning { kill(old.processIdentifier, SIGKILL) } }
    }
    deinit { cancel() }
}

// Optional alignment warms independently. Short opening sentences must not
// finish playing while the next audio chunk waits for a second model to load.
private final class SpeechAlignment {
    private let lock = NSLock()
    private var recognizer: OpaquePointer?
    init(library: String, model: String) {
        DispatchQueue.global(qos: .utility).async {
            let directory = URL(fileURLWithPath: model)
            guard ["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt", "bpe.vocab"].allSatisfy({ FileManager.default.isReadableFile(atPath: directory.appendingPathComponent($0).path) }) else { return }
            let recognizer = ht_create(library, model, 2)
            self.lock.lock(); self.recognizer = recognizer; self.lock.unlock()
        }
    }
    func json(samples: [Float]) -> String? {
        lock.lock(); let ready = recognizer; lock.unlock()
        guard let ready, let value = samples.withUnsafeBufferPointer({ ht_align(ready, $0.baseAddress, Int32($0.count)) }) else { return nil }
        defer { ht_free_text(value) }
        return String(cString: value)
    }
    deinit { if let recognizer { ht_destroy(recognizer) } }
}

func runSpeechWorker(model: String, voice: String, library: String, alignmentModel: String) throws {
    try Wire.prepare(STDIN_FILENO); try Wire.prepare(STDOUT_FILENO)
    let speech = PocketSpeech()
    let parentWatch = DispatchSource.makeProcessSource(identifier: getppid(), eventMask: .exit, queue: .global())
    parentWatch.setEventHandler { speech.stop(); _exit(0) }; parentWatch.resume()
    signal(SIGTERM, SIG_IGN)
    let termination = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .global())
    termination.setEventHandler { speech.stop(); _exit(0) }; termination.resume()
    defer { speech.stop(); parentWatch.cancel(); termination.cancel() }
    try speech.start(model: model, voice: voice)
    try Wire.write(Wire.json(["ready": true, "engine": "pocket-tts", "voice": voice]), to: STDOUT_FILENO)
    let alignment = SpeechAlignment(library: library, model: alignmentModel)
    while true {
        let request = try Wire.line(from: STDIN_FILENO, timeout: 86_400, limit: 16_384)
        guard let object = try JSONSerialization.jsonObject(with: request) as? [String: Any],
              let text = object["text"] as? String, !text.isEmpty, text.count <= 2_000 else { throw TalkError("Invalid speech chunk") }
        let audio = try speech.generate(text)
        let data = audio.samples.withUnsafeBytes { Data($0) }
        try Wire.write(Wire.json(["count": audio.samples.count, "rate": Int(audio.sampleRate)]), to: STDOUT_FILENO)
        try Wire.write(data, to: STDOUT_FILENO)
        let pcm = try speechAlignmentSamples(data, rate: audio.sampleRate)
        guard let json = alignment.json(samples: pcm) else {
            try Wire.write(Wire.json(["tokens": [], "timestamps": []]), to: STDOUT_FILENO); continue
        }
        var timing = Data(json.utf8); timing.append(10)
        try Wire.write(timing, to: STDOUT_FILENO)
    }
}

private func speechAlignmentSamples(_ data: Data, rate: Double) throws -> [Float] {
    guard let sourceFormat = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 1),
          let targetFormat = AVAudioFormat(standardFormatWithSampleRate: 16_000, channels: 1),
          let input = AVAudioPCMBuffer(pcmFormat: sourceFormat, frameCapacity: AVAudioFrameCount(data.count / 4)),
          let output = AVAudioPCMBuffer(pcmFormat: targetFormat, frameCapacity: AVAudioFrameCount(Double(data.count / 4) * 16_000 / rate) + 64),
          let converter = AVAudioConverter(from: sourceFormat, to: targetFormat) else { throw TalkError("Cannot align speech audio") }
    input.frameLength = input.frameCapacity
    _ = data.withUnsafeBytes { memcpy(input.floatChannelData![0], $0.baseAddress!, data.count) }
    var supplied = false, error: NSError?
    let status = converter.convert(to: output, error: &error) { _, outStatus in
        if supplied { outStatus.pointee = .endOfStream; return nil }
        supplied = true; outStatus.pointee = .haveData; return input
    }
    guard status != .error else { throw error ?? TalkError("Cannot align speech audio") as NSError }
    return Array(UnsafeBufferPointer(start: output.floatChannelData![0], count: Int(output.frameLength)))
}
