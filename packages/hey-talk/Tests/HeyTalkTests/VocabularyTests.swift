import XCTest
@testable import HeyTalk

final class VocabularyTests: XCTestCase {
    private func withVocabulary(_ body: (Vocabulary, URL) throws -> Void) throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("hey-talk-vocabulary-\(UUID())")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let defaults = directory.appendingPathComponent("defaults.md")
        try "Nova\nKamil\n".write(to: defaults, atomically: true, encoding: .utf8)
        try body(Vocabulary(file: directory.appendingPathComponent("settings/vocabulary.md"), defaults: defaults), defaults)
    }

    func testCommandsPersistPhrasesAndPreferredSpelling() throws {
        try withVocabulary { vocabulary, _ in
            XCTAssertEqual(try vocabulary.command([]), ["Kamil", "Nova"])
            XCTAssertEqual(try vocabulary.command(["add", "T3   Code", "Nova", "hey-boss"]), ["Kamil", "Nova", "T3 Code", "hey-boss"])
            XCTAssertEqual(try vocabulary.command(["add", "NOVA"]), ["Kamil", "NOVA", "T3 Code", "hey-boss"])
            let reloaded = Vocabulary(file: vocabulary.file, defaults: nil)
            XCTAssertEqual(try reloaded.command(["remove", "nova", "T3 Code"]), ["Kamil", "hey-boss"])
            XCTAssertEqual(try String(contentsOf: vocabulary.file, encoding: .utf8), "Kamil\nhey-boss\n")
            let permissions = try FileManager.default.attributesOfItem(atPath: vocabulary.file.path)[.posixPermissions] as? Int
            XCTAssertEqual(permissions, 0o600)
        }
    }

    func testEmptyListAndUserEditsSurviveChangedDefaults() throws {
        try withVocabulary { vocabulary, defaults in
            XCTAssertEqual(try vocabulary.command(["remove", "Nova", "Kamil"]), [])
            try "Another term\n".write(to: defaults, atomically: true, encoding: .utf8)
            XCTAssertEqual(try vocabulary.command([]), [])
            try "\n  Cloudflare  \nNova\nnova\n".write(to: vocabulary.file, atomically: true, encoding: .utf8)
            XCTAssertEqual(try vocabulary.words(), ["Cloudflare", "nova"])
        }
    }

    func testInvalidBatchDoesNotPartiallyChangeSavedVocabulary() throws {
        try withVocabulary { vocabulary, _ in
            let original = try vocabulary.command([])
            for args in [["add"], ["set", "Nova"], ["remove"], ["add", ""], ["add", "Valid", "one\ntwo"],
                         ["add", "one/two"], ["add", "Nova :99"], ["add", "\0"], ["remove", "Nova", "bad\rterm"]] {
                XCTAssertThrowsError(try vocabulary.command(args), "\(args)")
                XCTAssertEqual(try vocabulary.words(), original)
            }
        }
    }

    func testReadFailureNeverSilentlyResetsUserVocabulary() throws {
        try withVocabulary { vocabulary, _ in
            _ = try vocabulary.command([])
            try Data([0xff, 0xfe]).write(to: vocabulary.file)
            XCTAssertThrowsError(try vocabulary.command(["add", "Cloudflare"]))
            XCTAssertEqual(try Data(contentsOf: vocabulary.file), Data([0xff, 0xfe]))
        }
    }

    func testFirstWarmupAndConcurrentEditsKeepEveryTerm() throws {
        for _ in 0..<10 {
            try withVocabulary { vocabulary, _ in
                let operations = [[], ["add", "Cloudflare"], ["add", "Vercel"]]
                DispatchQueue.concurrentPerform(iterations: operations.count) { index in
                    do { _ = try vocabulary.command(operations[index]) }
                    catch { XCTFail(error.localizedDescription) }
                }
                XCTAssertEqual(try vocabulary.words(), ["Cloudflare", "Kamil", "Nova", "Vercel"])
            }
        }
    }
}
