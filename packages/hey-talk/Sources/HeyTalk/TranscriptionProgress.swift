import Foundation

struct TranscriptionProgress {
    let duration: Double
    init(audioSeconds: Double, correction: Bool) {
        // Local recognition is roughly 25x real time; allow for startup and a short API pass.
        duration = 0.6 + audioSeconds / 25 + (correction ? 1 + audioSeconds / 50 : 0)
    }
    func fraction(elapsed: Double) -> Double {
        // Approach completion gently if an estimate runs long; only a result finishes the rim.
        let time = max(0, elapsed) / duration
        return 0.98 * time / (time + 0.25)
    }
}
