import Foundation
import TalkCore

// The only synthesis backend. Both desktop readers use the speech worker's
// format-neutral PCM protocol; model and voice choices stay here in Hey Talk.
final class PocketSpeech {
    private let child = Process(), errors = Pipe()
    private let session = URLSession(configuration: .ephemeral)
    private var endpoint: URL?

    func start(model: String, voice: String) throws {
        let directory = URL(fileURLWithPath: model)
        child.executableURL = ModelPaths.contents.appendingPathComponent("Helpers/pocket-tts")
        child.currentDirectoryURL = directory
        child.arguments = ["serve", "--host", "127.0.0.1", "--port", "0",
            "--config", directory.appendingPathComponent("config.yaml").path,
            "--default-voice", directory.appendingPathComponent(voice + ".safetensors").path]
        child.standardInput = FileHandle.nullDevice; child.standardOutput = FileHandle.nullDevice
        child.standardError = errors
        child.environment = ProcessInfo.processInfo.environment.filter { !$0.key.hasPrefix("DYLD_") }
        try Wire.prepare(errors.fileHandleForReading.fileDescriptor)
        try child.run()
        let deadline = ProcessInfo.processInfo.systemUptime + 60
        while endpoint == nil {
            let remaining = deadline - ProcessInfo.processInfo.systemUptime
            guard remaining > 0 else { throw TalkError("Speech model took too long to load") }
            let line = String(decoding: try Wire.line(from: errors.fileHandleForReading.fileDescriptor, timeout: remaining, limit: 16_384), as: UTF8.self)
            if let marker = line.range(of: "Server running on "),
               let address = line[marker.upperBound...].split(whereSeparator: \.isWhitespace).first,
               let url = URL(string: String(address)),
               url.scheme == "http", url.host == "127.0.0.1", url.port != nil {
                endpoint = url.appendingPathComponent("tts")
            }
        }
        // Drain model diagnostics so a full stderr pipe cannot stall synthesis.
        errors.fileHandleForReading.readabilityHandler = { handle in
            if handle.availableData.isEmpty { handle.readabilityHandler = nil }
        }
    }

    func generate(_ text: String, chunk: @escaping (SpeechAudio) throws -> Void) throws -> SpeechAudio {
        guard let endpoint else { throw TalkError("Speech model is not ready") }
        var request = URLRequest(url: endpoint)
        request.httpMethod = "POST"; request.timeoutInterval = 60
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        let escaped = text.addingPercentEncoding(withAllowedCharacters: .alphanumerics)!
        request.httpBody = Data(("text=" + escaped).utf8)
        let response = StreamingWAV(chunk)
        let task = session.dataTask(with: request)
        task.delegate = response
        task.resume(); response.done.wait()
        if let error = response.error { throw error }
        return SpeechAudio(samples: response.samples, sampleRate: Double(response.rate))
    }

    func stop() {
        session.invalidateAndCancel()
        if child.isRunning { child.terminate(); child.waitUntilExit() }
        errors.fileHandleForReading.readabilityHandler = nil
    }
    deinit { stop() }
}

// URLSession serializes delegate calls. WAV payloads can split even a single PCM sample.
private final class StreamingWAV: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    let done = DispatchSemaphore(value: 0)
    private(set) var error: Error?
    private(set) var samples = [Float](), rate = 0
    private var pending = Data()
    private let chunk: (SpeechAudio) throws -> Void
    init(_ chunk: @escaping (SpeechAudio) throws -> Void) { self.chunk = chunk }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        guard error == nil else { return }
        do {
            guard (dataTask.response as? HTTPURLResponse)?.statusCode == 200 else { throw TalkError("Could not generate speech") }
            pending.append(data)
            if rate == 0 {
                guard pending.count >= 44 else { return }
                let h = Array(pending.prefix(44))
                func u16(_ i: Int) -> Int { Int(h[i]) | Int(h[i + 1]) << 8 }
                rate = u16(24) | u16(26) << 16
                guard String(decoding: h[0..<4], as: UTF8.self) == "RIFF",
                      String(decoding: h[8..<16], as: UTF8.self) == "WAVEfmt ",
                      String(decoding: h[36..<40], as: UTF8.self) == "data",
                      u16(20) == 1, u16(22) == 1, u16(34) == 16, (8_000...96_000).contains(rate)
                else { throw TalkError("Invalid speech WAV") }
                pending.removeFirst(44)
            }
            guard samples.count + pending.count / 2 <= rate * 120 else { throw TalkError("Invalid speech duration") }
            // Keep the last block until EOF so the player knows which buffer ends this passage.
            let frames = rate / 8
            while pending.count > frames * 2 { try deliver(frames, final: false) }
        } catch { self.error = error; dataTask.cancel() }
    }

    private func deliver(_ frames: Int, final: Bool) throws {
        let block: [Float] = pending.withUnsafeBytes { raw in
            (0..<frames).map { Float(Int16(littleEndian: raw.loadUnaligned(fromByteOffset: $0 * 2, as: Int16.self))) / 32768 }
        }
        pending.removeFirst(frames * 2)
        samples.append(contentsOf: block)
        try chunk(SpeechAudio(samples: block, sampleRate: Double(rate), final: final))
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError failure: Error?) {
        defer { done.signal() }
        if error == nil { error = failure }
        guard error == nil else { return }
        do {
            guard rate > 0, !pending.isEmpty, pending.count % 2 == 0 else { throw TalkError("Incomplete speech WAV") }
            try deliver(pending.count / 2, final: true)
        } catch { self.error = error }
    }
}
