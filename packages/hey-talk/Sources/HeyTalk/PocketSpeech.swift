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

    func generate(_ text: String) throws -> SpeechAudio {
        guard let endpoint else { throw TalkError("Speech model is not ready") }
        var request = URLRequest(url: endpoint)
        request.httpMethod = "POST"; request.timeoutInterval = 60
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        let escaped = text.addingPercentEncoding(withAllowedCharacters: .alphanumerics)!
        request.httpBody = Data(("text=" + escaped).utf8)
        let done = DispatchSemaphore(value: 0)
        var result: Result<Data, Error> = .failure(TalkError("Speech generation timed out"))
        let task = session.dataTask(with: request) { data, response, error in
            if let error { result = .failure(error) }
            else if (response as? HTTPURLResponse)?.statusCode == 200, let data { result = .success(data) }
            else { result = .failure(TalkError("Could not generate speech")) }
            done.signal()
        }
        task.resume(); done.wait()
        return try Self.decode(result.get())
    }

    static func decode(_ wav: Data) throws -> SpeechAudio {
        // Pocket streams a PCM16 mono WAV with an unknown length in its header.
        guard wav.count > 44, wav.count <= 44 + 96_000 * 120 * 2, (wav.count - 44) % 2 == 0,
              String(decoding: wav.prefix(4), as: UTF8.self) == "RIFF",
              String(decoding: wav[8..<16], as: UTF8.self) == "WAVEfmt ",
              String(decoding: wav[36..<40], as: UTF8.self) == "data" else { throw TalkError("Invalid speech WAV") }
        return try wav.withUnsafeBytes { bytes in
            func u16(_ offset: Int) -> UInt16 { UInt16(littleEndian: bytes.loadUnaligned(fromByteOffset: offset, as: UInt16.self)) }
            let rate = Int(UInt32(littleEndian: bytes.loadUnaligned(fromByteOffset: 24, as: UInt32.self)))
            guard u16(20) == 1, u16(22) == 1, u16(34) == 16, (8_000...96_000).contains(rate),
                  (wav.count - 44) / 2 <= rate * 120 else { throw TalkError("Unsupported speech WAV") }
            let samples = stride(from: 44, to: wav.count, by: 2).map { Float(Int16(bitPattern: u16($0))) / 32_768 }
            return SpeechAudio(samples: samples, sampleRate: Double(rate))
        }
    }

    func stop() {
        session.invalidateAndCancel()
        if child.isRunning { child.terminate(); child.waitUntilExit() }
        errors.fileHandleForReading.readabilityHandler = nil
    }
    deinit { stop() }
}
