import AppKit

struct SpeechParagraph: Equatable {
    let range: NSRange
    let frame: CGRect
}

enum SpeechPassage {
    static func join(_ passages: [SpeechTarget]) -> SpeechTarget {
        precondition(!passages.isEmpty)
        let rich = NSMutableAttributedString(string: "")
        var paragraphs: [SpeechParagraph] = []
        for passage in passages.prefix(64) {
            let separator = rich.length == 0 ? "" : "\n\n"
            guard rich.length + separator.utf16.count + passage.text.utf16.count <= 20_000 else { break }
            rich.append(NSAttributedString(string: separator))
            let range = NSRange(location: rich.length, length: passage.text.utf16.count)
            rich.append(passage.attributed ?? NSAttributedString(string: passage.text))
            paragraphs.append(SpeechParagraph(range: range, frame: passage.frame))
        }
        let first = passages[0]
        return SpeechTarget(text: rich.string, frame: first.frame, point: first.point, pid: first.pid,
                            bundleID: first.bundleID, source: first.source, attributed: rich, paragraphs: paragraphs)
    }
}
