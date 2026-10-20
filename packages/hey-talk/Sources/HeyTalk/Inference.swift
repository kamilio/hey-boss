import Foundation
import CSherpa
import TalkCore
import Darwin

enum ModelPaths {
    static var directory: URL {
        let installed = ControlSocket.directory.appendingPathComponent("models/orukeet-v0.1.0")
        if FileManager.default.fileExists(atPath: installed.appendingPathComponent("encoder.int8.onnx").path) { return installed }
        // Reuse an existing download during development; the on-demand installer
        // makes an independent copy so removing OpenWhispr cannot break Hey Talk.
        return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".cache/openwhispr/parakeet-models/orukeet-v0.1.0")
    }
    static var library: URL { Bundle.main.bundleURL.appendingPathComponent("Contents/Frameworks/libsherpa-onnx-c-api.dylib") }
    static var vocabulary: URL { Bundle.main.bundleURL.appendingPathComponent("Contents/Resources/vocabulary.md") }
    static var available: Bool {
        ["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt", "bpe.vocab"].allSatisfy {
            FileManager.default.fileExists(atPath: directory.appendingPathComponent($0).path)
        } && FileManager.default.fileExists(atPath: library.path)
          && FileManager.default.isReadableFile(atPath: vocabulary.path)
    }
}

// A persistent native worker owns ONNX. Pipes are private; no HTTP listener,
// WebSocket server, JavaScript runtime, or model state in the desktop process.
final class Inference {
    private let queue = DispatchQueue(label: "hey-talk.inference", qos: .userInitiated)
    private let lock = NSLock()
    private var process: Process?
    private var input: Pipe?
    private var output: Pipe?
    private var epoch: UInt64 = 0
    var pid: Int32? { lock.lock(); defer { lock.unlock() }; return process?.isRunning == true ? process?.processIdentifier : nil }

    func warm(completion: @escaping (Result<Void, Error>) -> Void = { _ in }) {
        lock.lock(); let ticket = epoch; lock.unlock()
        queue.async {
            do { try self.ensureWorker(ticket: ticket); completion(.success(())) }
            catch { self.cancel(ifEpoch: ticket); completion(.failure(error)) }
        }
    }
    func transcribe(_ samples: [Float], completion: @escaping (Result<(String, Double), Error>) -> Void) {
        lock.lock(); let ticket = epoch; lock.unlock()
        queue.async {
            for attempt in 0...1 {
                do {
                    try self.ensureWorker(ticket: ticket)
                    self.lock.lock()
                    let input = self.input; let output = self.output; let valid = ticket == self.epoch
                    self.lock.unlock()
                    guard valid, let input, let output else { throw TalkError("Transcription cancelled") }
                    let start = ProcessInfo.processInfo.systemUptime
                    try Wire.write(PCMFrame.encode(samples), to: input.fileHandleForWriting.fileDescriptor, timeout: 60)
                    let line = try Wire.line(from: output.fileHandleForReading.fileDescriptor, timeout: 120)
                    guard let result = try JSONSerialization.jsonObject(with: line) as? [String: Any],
                          let text = result["text"] as? String else { throw TalkError("Inference returned an invalid result") }
                    completion(.success((text.trimmingCharacters(in: .whitespacesAndNewlines), ProcessInfo.processInfo.systemUptime - start)))
                    return
                } catch {
                    self.lock.lock(); let cancelled = ticket != self.epoch; self.lock.unlock()
                    // A worker can exit after isRunning is checked. Retrying this
                    // pure local decode once is safe; cancellation never retries.
                    if attempt == 0, !cancelled, error.localizedDescription.hasPrefix("Connection") {
                        self.discardWorker(ifEpoch: ticket)
                        continue
                    }
                    self.cancel(ifEpoch: ticket)
                    completion(.failure(error))
                    return
                }
            }
        }
    }
    private func ensureWorker(ticket: UInt64) throws {
        lock.lock()
        defer { lock.unlock() }
        guard ticket == epoch else { throw TalkError("Transcription cancelled") }
        if process?.isRunning == true { return }
        guard ModelPaths.available else { throw TalkError("Orukeet is missing. Run the hey-talk installer to prepare its model and runtime.") }
        let child = Process(), stdin = Pipe(), stdout = Pipe()
        child.executableURL = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/hey-talk-inference")
        child.arguments = ["--worker", ModelPaths.directory.path, ModelPaths.library.path, try Vocabulary().prepare().path]
        child.standardInput = stdin; child.standardOutput = stdout
        child.standardError = FileHandle.standardError
        child.environment = ProcessInfo.processInfo.environment.filter { !$0.key.hasPrefix("DYLD_") }
        try Wire.prepare(stdin.fileHandleForWriting.fileDescriptor)
        try Wire.prepare(stdout.fileHandleForReading.fileDescriptor)
        try child.run()
        process = child; input = stdin; output = stdout
        // Do not hold the lock while the worker loads: cancel can terminate it.
        lock.unlock()
        let ready: Data
        do { ready = try Wire.line(from: stdout.fileHandleForReading.fileDescriptor, timeout: 60) }
        catch { lock.lock(); throw error }
        lock.lock()
        guard ticket == epoch else { throw TalkError("Transcription cancelled") }
        let message = try JSONSerialization.jsonObject(with: ready) as? [String: Any]
        guard message?["ready"] as? Bool == true else { throw TalkError(message?["error"] as? String ?? "Orukeet failed to initialize") }
    }
    func cancel(ifEpoch expected: UInt64? = nil) {
        discardWorker(ifEpoch: expected, invalidate: true)
    }
    private func discardWorker(ifEpoch expected: UInt64? = nil, invalidate: Bool = false) {
        lock.lock()
        if let expected, expected != epoch { lock.unlock(); return }
        if invalidate { epoch &+= 1 }
        let old = process
        process = nil; input = nil; output = nil
        lock.unlock()
        guard let old, old.isRunning else { return }
        old.terminate()
        DispatchQueue.global().asyncAfter(deadline: .now() + 1) {
            if old.isRunning { kill(old.processIdentifier, SIGKILL) }
        }
    }
    deinit { cancel() }
}

func runInferenceWorker(model: String, library: String, vocabulary: String) throws {
    guard FileManager.default.isReadableFile(atPath: vocabulary),
          FileManager.default.isReadableFile(atPath: URL(fileURLWithPath: model).appendingPathComponent("bpe.vocab").path) else {
        throw TalkError("Vocabulary files are missing. Run the hey-talk installer.")
    }
    try Wire.prepare(STDIN_FILENO)
    try Wire.prepare(STDOUT_FILENO)
    // Exit if the recording service crashes while inference is in flight.
    let parent = getppid()
    let parentWatch = DispatchSource.makeProcessSource(identifier: parent, eventMask: .exit, queue: .global())
    parentWatch.setEventHandler { _exit(0) }
    parentWatch.resume()
    let terms = Vocabulary(file: URL(fileURLWithPath: vocabulary), defaults: nil)
    _ = try terms.words()
    guard let recognizer = ht_create(library, model, 4) else { throw TalkError(String(cString: ht_error())) }
    defer { ht_destroy(recognizer); parentWatch.cancel() }
    try Wire.write(Wire.json(["ready": true]), to: STDOUT_FILENO)
    while true {
        let header = try Wire.readExactly(4, from: STDIN_FILENO, timeout: 86_400)
        let length = header.withUnsafeBytes { Int($0.loadUnaligned(as: UInt32.self).littleEndian) }
        guard length > 0, length <= PCMFrame.maxSamples * 4, length % 4 == 0 else { throw TalkError("Invalid audio length") }
        let payload = try Wire.readExactly(length, from: STDIN_FILENO)
        let samples = try PCMFrame.decode(header + payload)
        // Snapshot the latest list for this utterance; updates never reload the model
        // or interrupt an in-flight decode.
        let hotwords = try terms.words().joined(separator: "/")
        guard let result = samples.withUnsafeBufferPointer({ ht_transcribe(recognizer, $0.baseAddress, Int32($0.count), hotwords) }) else {
            throw TalkError("Orukeet could not decode audio")
        }
        let text = String(cString: result)
        ht_free_text(result)
        try Wire.write(Wire.json(["text": text]), to: STDOUT_FILENO)
    }
}
