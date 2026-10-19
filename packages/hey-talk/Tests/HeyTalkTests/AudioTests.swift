import XCTest
import AVFoundation
@testable import HeyTalk

final class AudioTests: XCTestCase {
    func testFileResamplingPreservesDurationAndFiniteSamples() throws {
        for (rate, channels) in [(16_000.0, 1), (22_050.0, 1), (44_100.0, 2), (48_000.0, 2), (96_000.0, 1)] {
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("hey-talk-test-\(UUID()).caf")
            defer { try? FileManager.default.removeItem(at: url) }
            let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: rate, channels: AVAudioChannelCount(channels), interleaved: false)!
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(rate))!
            buffer.frameLength = buffer.frameCapacity
            for channel in 0..<channels {
                for i in 0..<Int(buffer.frameLength) { buffer.floatChannelData![channel][i] = Float(sin(Double(i) * 2 * .pi * 440 / rate) * 0.2) }
            }
            do {
                let file = try AVAudioFile(forWriting: url, settings: format.settings)
                try file.write(from: buffer)
            }
            let samples = try AudioCapture.readFile(url.path)
            XCTAssertEqual(samples.count, 16_000, accuracy: 2, "rate \(rate)")
            XCTAssertTrue(samples.allSatisfy(\.isFinite))
            XCTAssertGreaterThan(samples.map { abs($0) }.max() ?? 0, 0.1)
        }
    }
    func testEmptyAndOverlongAudioFailCleanly() throws {
        let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false)!
        for seconds in [0, 301] {
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("hey-talk-test-\(UUID()).caf")
            defer { try? FileManager.default.removeItem(at: url) }
            do {
                let file = try AVAudioFile(forWriting: url, settings: format.settings)
                if seconds > 0 {
                    let silence = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 16_000)!
                    silence.frameLength = 16_000
                    memset(silence.floatChannelData![0], 0, 16_000 * MemoryLayout<Float>.size)
                    for _ in 0..<seconds { try file.write(from: silence) }
                }
            }
            XCTAssertThrowsError(try AudioCapture.readFile(url.path))
        }
    }
}
