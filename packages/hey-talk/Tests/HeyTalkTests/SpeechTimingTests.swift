import XCTest
@testable import HeyTalk

final class SpeechTimingTests: XCTestCase {
    func testSherpaSpacePrefixedSubwordsAreJoinedBeforeMatching() {
        let words = SpeechTiming.align(text: "Hello there. This paragraph", tokens: [" H", "ello", " there", ".", " This", " par", "ag", "ra", "ph"], times: [0.16, 0.4, 0.72, 1.04, 1.2, 1.44, 1.6, 1.68, 1.84], duration: 2.1)
        XCTAssertEqual(words.map(\.range), [NSRange(location: 0, length: 5), NSRange(location: 6, length: 5), NSRange(location: 13, length: 4), NSRange(location: 18, length: 9)])
        XCTAssertEqual(words.map(\.start), [0.16, 0.72, 1.2, 1.44])
    }
    func testFirstPhraseIsShortAndRangesPreserveOriginalPassage() throws {
        let text = "This is quite a long first sentence with enough words to test fast streaming playback without waiting for it all.\n\nAnother paragraph."
        let parts = try SpeechText.segments(text)
        XCTAssertLessThanOrEqual(parts[0].text.count, 60)
        for part in parts { XCTAssertEqual((text as NSString).substring(with: part.range), part.text) }
        XCTAssertEqual(parts.map(\.text).joined(separator: " "), text.split(whereSeparator: \.isWhitespace).joined(separator: " "))
    }
    func testTokenTimingsMapToOriginalWordsIncludingBoldSourceTextAndUnicode() {
        let words = SpeechTiming.align(text: "Hello, café world!", tokens: ["▁Hello", ",", "▁ca", "fé", "▁world", "!"], times: [0.1, 0.2, 0.4, 0.5, 0.8, 1.0], duration: 1.2)
        XCTAssertEqual(words.map(\.range), [NSRange(location: 0, length: 5), NSRange(location: 7, length: 4), NSRange(location: 12, length: 5)])
        XCTAssertEqual(words.map(\.start), [0.1, 0.4, 0.8])
    }
    func testMismatchedRecognitionDoesNotInventWordTiming() {
        let words = SpeechTiming.align(text: "Read PR 20624 now.", tokens: ["▁Read", "▁pull", "▁request", "▁now"], times: [0, 0.2, 0.4, 1.0], duration: 1.4)
        XCTAssertEqual(words.map(\.range), [NSRange(location: 0, length: 4), NSRange(location: 14, length: 3)])
        XCTAssertTrue(SpeechTiming.align(text: "Hello", tokens: ["▁Hello"], times: [], duration: 1).isEmpty)
    }
}
