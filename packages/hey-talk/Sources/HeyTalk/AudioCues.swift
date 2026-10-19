import AppKit

// OpenWhispr's two-note cues, synthesized once as native PCM instead of Web Audio.
final class AudioCues {
    private let startSound = AudioCues.make([523.25, 659.25])
    private let stopSound = AudioCues.make([587.33, 440])
    func start() { startSound?.stop(); startSound?.play() }
    func stop() { stopSound?.stop(); stopSound?.play() }
    private static func make(_ notes: [Double]) -> NSSound? {
        let rate = 44_100.0, duration = 0.09, gap = 0.025, attack = 0.015
        let length = Int((Double(notes.count) * (duration + gap)) * rate)
        var pcm = Data(capacity: length * 2)
        for index in 0..<length {
            let time = Double(index) / rate
            let note = min(notes.count - 1, Int(time / (duration + gap)))
            let local = time - Double(note) * (duration + gap)
            let gain = local > duration ? 0 : local < attack ? 0.0001 + 0.1999 * local / attack : 0.2 * pow(0.0001 / 0.2, (local - attack) / (duration - attack))
            var sample = Int16(sin(2 * .pi * notes[note] * local) * gain * 32767).littleEndian
            pcm.append(Data(bytes: &sample, count: 2))
        }
        var wav = Data("RIFF".utf8)
        func integer<T: FixedWidthInteger>(_ value: T) { var v = value.littleEndian; wav.append(Data(bytes: &v, count: MemoryLayout<T>.size)) }
        integer(UInt32(36 + pcm.count)); wav.append(Data("WAVEfmt ".utf8)); integer(UInt32(16))
        integer(UInt16(1)); integer(UInt16(1)); integer(UInt32(rate)); integer(UInt32(rate * 2))
        integer(UInt16(2)); integer(UInt16(16)); wav.append(Data("data".utf8)); integer(UInt32(pcm.count)); wav.append(pcm)
        return NSSound(data: wav)
    }
}
