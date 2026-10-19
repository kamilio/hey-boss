import XCTest
import AVFoundation
@testable import HeyTalk

final class AudioTests: XCTestCase {
    func testFileResamplingPreservesDurationAndFiniteSamples() throws {
        for rate in [16_000.0, 22_050.0, 48_000.0] {
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("hey-talk-test-\(UUID()).caf")
            defer { try? FileManager.default.removeItem(at: url) }
            let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: rate, channels: 1, interleaved: false)!
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(rate))!
            buffer.frameLength = buffer.frameCapacity
            for i in 0..<Int(buffer.frameLength) { buffer.floatChannelData![0][i] = Float(sin(Double(i) * 2 * .pi * 440 / rate) * 0.2) }
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
}
