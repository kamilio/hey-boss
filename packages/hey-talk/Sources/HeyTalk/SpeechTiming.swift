import Foundation
import TalkCore

struct SpeechSegment {
    let text: String
    let range: NSRange
}
struct SpeechWord {
    let range: NSRange
    let start: Double
    let end: Double
}

enum SpeechText {
    static func chunks(_ text: String) throws -> [String] { try segments(text).map(\.text) }
    static func segments(_ text: String) throws -> [SpeechSegment] {
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { throw TalkError("No readable text here") }
        guard text.utf16.count <= 20_000 else { throw TalkError("Select a shorter passage (up to 20,000 UTF-16 units)") }
        var result: [SpeechSegment] = []
        text.enumerateSubstrings(in: text.startIndex..<text.endIndex, options: .bySentences) { _, range, _, _ in
            var begin = range.lowerBound
            while begin < range.upperBound {
                while begin < range.upperBound && text[begin].isWhitespace { begin = text.index(after: begin) }
                guard begin < range.upperBound else { break }
                let limit = result.isEmpty ? 60 : 180
                var end = text.index(begin, offsetBy: limit, limitedBy: range.upperBound) ?? range.upperBound
                if end < range.upperBound, let space = text[begin..<end].lastIndex(where: \.isWhitespace) { end = space }
                var trimmedEnd = end
                while trimmedEnd > begin && text[text.index(before: trimmedEnd)].isWhitespace { trimmedEnd = text.index(before: trimmedEnd) }
                if trimmedEnd > begin {
                    let part = begin..<trimmedEnd
                    result.append(SpeechSegment(text: String(text[part]), range: NSRange(part, in: text)))
                }
                begin = end
            }
        }
        return result
    }
}

enum SpeechTiming {
    // Match recognized words to the immutable source using an ordered alignment.
    // Numbers and code may be pronounced differently: leave those words unmarked
    // rather than fabricate precise timings. The current phrase still stays visible.
    static func align(text: String, tokens: [String], times: [Double], duration: Double) -> [SpeechWord] {
        guard tokens.count == times.count, !tokens.isEmpty else { return [] }
        var recognized: [(String, Double)] = []
        let bpe = tokens.contains { $0.contains("▁") || $0.hasPrefix(" ") }
        var word = "", start = 0.0
        func flush() {
            let key = normalize(word)
            if !key.isEmpty { recognized.append((key, max(0, start))) }
            word = ""
        }
        for (token, time) in zip(tokens, times) {
            guard time.isFinite, time >= 0, time <= duration else { continue }
            if token.hasPrefix("▁") || token.hasPrefix(" ") || !bpe { flush(); start = time }
            if word.isEmpty { start = time }
            word += token.replacingOccurrences(of: "▁", with: "").trimmingCharacters(in: .whitespaces)
        }
        flush()
        var source: [(String, NSRange)] = []
        text.enumerateSubstrings(in: text.startIndex..<text.endIndex, options: .byWords) { word, range, _, _ in
            if let word { source.append((normalize(word), NSRange(range, in: text))) }
        }
        let n = source.count, m = recognized.count
        guard n > 0, m > 0, n <= 240, m <= 512 else { return [] }
        var scores = Array(repeating: Array(repeating: 0, count: m + 1), count: n + 1)
        for i in (0..<n).reversed() { for j in (0..<m).reversed() {
            scores[i][j] = source[i].0 == recognized[j].0 ? scores[i + 1][j + 1] + 1 : max(scores[i + 1][j], scores[i][j + 1])
        } }
        var i = 0, j = 0, words: [SpeechWord] = []
        while i < n && j < m {
            if source[i].0 == recognized[j].0 {
                words.append(SpeechWord(range: source[i].1, start: recognized[j].1,
                    end: j + 1 < m ? max(recognized[j].1, recognized[j + 1].1) : duration))
                i += 1; j += 1
            } else if scores[i + 1][j] >= scores[i][j + 1] { i += 1 } else { j += 1 }
        }
        return words
    }
    private static func normalize(_ text: String) -> String {
        String(text.lowercased().unicodeScalars.filter { CharacterSet.alphanumerics.contains($0) })
    }
}
