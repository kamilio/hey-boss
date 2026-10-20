import Foundation

public struct TalkError: LocalizedError {
    public let message: String
    public init(_ message: String) { self.message = message }
    public var errorDescription: String? { message }
}

public enum Phase: String, Codable { case idle, recording, processing, failed }

public struct RecordingSession {
    public private(set) var phase: Phase = .idle
    public private(set) var generation: UInt64 = 0
    public private(set) var lastText: String = ""
    public init() {}
    public mutating func begin() throws -> UInt64 {
        guard phase == .idle || phase == .failed else { throw TalkError("Already \(phase.rawValue)") }
        generation &+= 1
        phase = .recording
        return generation
    }
    public mutating func processing(_ id: UInt64) throws {
        guard id == generation, phase == .recording else { throw TalkError("Recording ended") }
        phase = .processing
    }
    @discardableResult public mutating func finish(_ id: UInt64, text: String) -> Bool {
        guard id == generation, phase == .processing else { return false }
        if !text.isEmpty { lastText = text }
        phase = .idle
        return true
    }
    public mutating func cancel() { generation &+= 1; phase = .idle }
    public mutating func fail(_ id: UInt64) {
        guard id == generation else { return }
        phase = .failed
    }
}

// Geometry and signal response ported from OpenWhispr (MIT; see THIRD_PARTY.md).
public enum Waveform {
    public static let count = 11
    public static let resting: [Double] = [6, 12, 5, 9, 7, 22, 18, 5, 20, 12, 17]
    public static func height(_ rms: Double) -> Double {
        guard rms.isFinite, rms > 0 else { return 4 }
        return 4 + 18 * pow(min(1, rms * 8), 0.75)
    }
}

public enum DictationShortcut: Int, CaseIterable {
    case paste = 14, submit = 15
    public var keyCode: Int { rawValue }
    public var name: String { "Control+Command+Option+Shift+\(self == .paste ? "E" : "R")" }
    public static let hint = "Caps Lock + E / R"
    public static let help = "Hyper+E: paste · Hyper+R: paste + Enter"
}

public enum HotkeyAction: Equatable { case none, toggle(DictationShortcut), toggleAuto, cancel }
public struct HotkeyState {
    private var held: Int?
    public init() {}
    public mutating func keyDown(code: Int, hyper: Bool, repeated: Bool) -> HotkeyAction {
        if code == 53 { return repeated ? .none : .cancel }
        guard hyper, !repeated, held == nil else { return .none }
        if code == 9 { held = code; return .toggleAuto }
        guard let shortcut = DictationShortcut(rawValue: code) else { return .none }
        held = code
        return .toggle(shortcut)
    }
    public mutating func keyUp(code: Int) -> HotkeyAction {
        if code == held { held = nil }
        return .none
    }
}

public enum PCMFrame {
    public static let rate = 16_000
    public static let maxSamples = rate * 300
    public static func encode(_ samples: [Float]) throws -> Data {
        guard !samples.isEmpty, samples.count <= maxSamples, samples.allSatisfy({ $0.isFinite }) else {
            throw TalkError("Audio must contain 1–\(maxSamples) finite samples (maximum 5 minutes)")
        }
        var length = UInt32(samples.count * 4).littleEndian
        var data = Data(bytes: &length, count: 4)
        samples.withUnsafeBytes { data.append(contentsOf: $0) }
        return data
    }
    public static func decode(_ data: Data) throws -> [Float] {
        guard data.count >= 4 else { throw TalkError("Truncated audio header") }
        let count = data.withUnsafeBytes { Int($0.loadUnaligned(as: UInt32.self).littleEndian) }
        guard count > 0, count % 4 == 0, count <= maxSamples * 4, data.count == count + 4 else {
            throw TalkError("Invalid audio frame size")
        }
        let values = data.dropFirst(4).withUnsafeBytes { bytes in
            stride(from: 0, to: count, by: 4).map { bytes.loadUnaligned(fromByteOffset: $0, as: Float.self) }
        }
        guard values.allSatisfy({ $0.isFinite }) else { throw TalkError("Non-finite audio sample") }
        return values
    }
}
