import AVFoundation
import TalkCore

enum RecordingArchive {
    static func save(_ samples: [Float], in directory: URL) throws {
        guard !samples.isEmpty else { return }
        let files = FileManager.default
        try files.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        var info = stat()
        guard lstat(directory.path, &info) == 0, info.st_uid == getuid(), info.st_mode & S_IFMT == S_IFDIR,
              chmod(directory.path, 0o700) == 0 else { throw TalkError("Unsafe recordings directory") }
        var folder = directory
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try folder.setResourceValues(values)

        let names = try files.contentsOfDirectory(atPath: directory.path)
        let numbers = names.compactMap { name -> Int? in
            guard name.hasPrefix("recording-"), name.hasSuffix(".wav"),
                  let number = Int(name.dropFirst(10).dropLast(4)), number >= 1,
                  name == "recording-\(number).wav" else { return nil }
            return number
        }.sorted()
        func url(_ number: Int) -> URL { directory.appendingPathComponent("recording-\(number).wav") }
        let last = numbers.last ?? 0
        guard last < Int.max else { throw TalkError("Recording number limit reached") }
        let format = AVAudioFormat(standardFormatWithSampleRate: Double(PCMFrame.rate), channels: 1)!
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(samples.count))!
        buffer.frameLength = buffer.frameCapacity
        samples.withUnsafeBufferPointer { buffer.floatChannelData![0].update(from: $0.baseAddress!, count: $0.count) }

        // Evict before opening the WAV so even an interrupted write cannot leave a sixth recording.
        for old in numbers.dropLast(4) {
            guard unlink(url(old).path) == 0 else { throw TalkError("Cannot remove old recording") }
        }
        let next = url(last + 1)
        do {
            let file = try AVAudioFile(forWriting: next, settings: format.settings)
            try files.setAttributes([.posixPermissions: 0o600], ofItemAtPath: next.path)
            try file.write(from: buffer)
        } catch {
            unlink(next.path)
            throw error
        }
    }
}
