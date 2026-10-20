import XCTest
import TalkCore
@testable import HeyTalk

final class SharedVocabularyTests: XCTestCase {
    private func fixture(_ body: (Vocabulary, URL) throws -> Void) throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("hey-talk-shared-\(UUID())")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let defaults = root.appendingPathComponent("defaults.md")
        try "Kamil\nPoe\n".write(to: defaults, atomically: true, encoding: .utf8)
        try body(Vocabulary(file: root.appendingPathComponent("cache/vocabulary.md"), defaults: defaults), root)
    }

    func testExistingSharedListWinsIncludingAnEmptyList() throws {
        try fixture { cache, _ in
            _ = try cache.command(["add", "ans"])
            let shared = SharedVocabulary(cache: cache) { args in
                XCTAssertEqual(args, [])
                return ["ok": true, "initialized": true, "vocabulary": [String]()]
            }
            let result = try shared.command([])
            XCTAssertEqual(result["source"] as? String, "fleet")
            XCTAssertEqual(try cache.words(), [])
        }
    }

    func testMigrationKeepsPersonalWordsAndRefreshUsesOtherMachineEdits() throws {
        try fixture { cache, _ in
            _ = try cache.command(["add", "ans"])
            var words: [String]? = nil
            let shared = SharedVocabulary(cache: cache) { args in
                if args.first == "initialize" {
                    XCTAssertNil(words)
                    XCTAssertEqual(args[1], "--file")
                    words = try Vocabulary(file: URL(fileURLWithPath: args[2]), defaults: nil).words()
                } else if args.first == "add" { words!.append(contentsOf: args.dropFirst()) }
                return ["ok": true, "initialized": words != nil, "vocabulary": words ?? []]
            }
            XCTAssertEqual(try shared.command([])["vocabulary"] as? [String], ["Kamil", "Poe", "ans"])
            _ = try shared.command(["add", "Vercel"])
            XCTAssertTrue(try cache.words().contains("Vercel"))
            words = ["Cloudflare"] // A change arriving from another installation.
            _ = try shared.command([])
            XCTAssertEqual(try cache.words(), ["Cloudflare"])
        }
    }

    func testUnavailableOrInvalidSharedStoreKeepsLastGoodCacheAndRejectsEdits() throws {
        try fixture { cache, _ in
            _ = try cache.command(["add", "ans"])
            let original = try Data(contentsOf: cache.file)
            let offline = SharedVocabulary(cache: cache) { _ in throw TalkError("Sync unavailable") }
            let result = try offline.command([])
            XCTAssertEqual(result["source"] as? String, "cache")
            XCTAssertNotNil(result["sync_error"])
            XCTAssertThrowsError(try offline.command(["add", "Vercel"]))
            XCTAssertEqual(try Data(contentsOf: cache.file), original)
            let invalid = SharedVocabulary(cache: cache) { _ in
                ["ok": true, "initialized": true, "vocabulary": ["bad/term"]]
            }
            XCTAssertNotNil(try invalid.command([])["sync_error"])
            XCTAssertEqual(try Data(contentsOf: cache.file), original)
        }
    }

    func testInvalidBatchNeverContactsTheSharedStore() throws {
        try fixture { cache, _ in
            let shared = SharedVocabulary(cache: cache) { _ in
                XCTFail("Invalid command reached shared storage")
                return [:]
            }
            XCTAssertThrowsError(try shared.command(["add", "Valid", "bad/term"]))
        }
    }
}
