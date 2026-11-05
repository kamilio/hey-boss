import Foundation
import TalkCore

/// Uses the same Rust detector and turn boundaries as voice conversations.
final class SpeechDetector {
    enum Event { case began, ended, samples([Float]) }
    private let worker = Process(), input = Pipe(), output = Pipe()
    private let lock = NSLock()
    private var stopped = false

    init(receive: @escaping (Event) -> Void, failed: @escaping (String) -> Void) throws {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let paths = (ProcessInfo.processInfo.environment["PATH"] ?? "").split(separator: ":").map(String.init)
            + [home + "/.local/bin", "/opt/homebrew/bin", "/usr/local/bin", home + "/.cargo/bin"]
        guard let binary = paths.map({ URL(fileURLWithPath: $0).appendingPathComponent("hey-boss") })
            .first(where: { FileManager.default.isExecutableFile(atPath: $0.path) }) else {
            throw TalkError("Install Hey Boss to use Auto speech detection.")
        }
        worker.executableURL = binary; worker.arguments = ["review-audio", "--conversation"]
        worker.standardInput = input; worker.standardOutput = output; worker.standardError = FileHandle.nullDevice
        try worker.run()
        let writer = input.fileHandleForWriting, reader = output.fileHandleForReading
        _ = fcntl(writer.fileDescriptor, F_SETNOSIGPIPE, 1)
        _ = fcntl(writer.fileDescriptor, F_SETFL, fcntl(writer.fileDescriptor, F_GETFL) | O_NONBLOCK)
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            defer { try? reader.close() }
            func read(_ count: Int) throws -> Data {
                var bytes = Data()
                while bytes.count < count {
                    guard let part = try reader.read(upToCount: count - bytes.count), !part.isEmpty else { throw TalkError("Speech detector disconnected.") }
                    bytes.append(part)
                }
                return bytes
            }
            do {
                while true {
                    let header = try read(2)
                    switch Int(header[0]) + Int(header[1]) * 256 {
                    case 0: receive(.began)
                    case 1: receive(.ended)
                    case 2: break // Early speech candidate; wait for confirmed speech.
                    case 640:
                        let pcm = try read(640)
                        receive(.samples(pcm.withUnsafeBytes { bytes in
                            stride(from: 0, to: 640, by: 2).map { Float(Int16(littleEndian: bytes.loadUnaligned(fromByteOffset: $0, as: Int16.self))) / 32768 }
                        }))
                    default: throw TalkError("Invalid speech detector response. Update Hey Boss.")
                    }
                }
            } catch {
                guard let self else { return }
                self.lock.lock(); let stopped = self.stopped; self.lock.unlock()
                if !stopped { failed(error.localizedDescription) }
            }
        }
    }

    func send(_ samples: [Float]) throws {
        // Conversations use 16 kHz signed PCM; capture and transcription use Float.
        let pcm = samples.map { Int16(max(-32768, min(32767, ($0 * 32768).rounded()))).littleEndian }
        lock.lock(); defer { lock.unlock() }
        guard !stopped else { return }
        guard pcm.withUnsafeBytes({ Darwin.write(input.fileHandleForWriting.fileDescriptor, $0.baseAddress, $0.count) == $0.count }) else {
            throw TalkError("Speech detector cannot keep up with the microphone. Start again.")
        }
    }

    func stop() {
        lock.lock(); defer { lock.unlock() }
        guard !stopped else { return }
        stopped = true
        try? input.fileHandleForWriting.close()
        if worker.isRunning { worker.terminate() }
    }
    deinit { stop() }
}
