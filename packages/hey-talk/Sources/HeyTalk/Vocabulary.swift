import Foundation
import TalkCore

struct Vocabulary {
    let file: URL
    private let defaults: URL?

    init(file: URL = ControlSocket.directory.appendingPathComponent("vocabulary.md"),
         defaults: URL? = ModelPaths.vocabulary) {
        self.file = file; self.defaults = defaults
    }

    private static func term(_ value: String) throws -> String {
        let forbidden = CharacterSet.controlCharacters.union(CharacterSet(charactersIn: "/:"))
        guard value.rangeOfCharacter(from: forbidden) == nil else {
            throw TalkError("Vocabulary terms cannot contain control characters, slashes, or colons.")
        }
        let word = value.split(whereSeparator: \.isWhitespace).joined(separator: " ").precomposedStringWithCanonicalMapping
        guard !word.isEmpty, word.utf8.count <= 256 else { throw TalkError("Use a vocabulary word or phrase of 1–256 bytes.") }
        return word
    }

    static func validate(_ arguments: [String]) throws {
        guard !arguments.isEmpty else { return }
        guard arguments.count >= 2, ["add", "remove"].contains(arguments[0]) else {
            throw TalkError("Use vocabulary [add|remove WORD…]. Quote phrases, e.g. vocabulary add \"T3 Code\".")
        }
        for value in arguments.dropFirst() { _ = try term(value) }
    }

    func words() throws -> [String] {
        let source = FileManager.default.fileExists(atPath: file.path) ? file : defaults
        guard let source else { throw TalkError("Vocabulary file is missing. Run hey-talk vocabulary to recreate it.") }
        let text = try String(contentsOf: source, encoding: .utf8)
        var words: [String: String] = [:]
        for line in text.components(separatedBy: .newlines) where !line.trimmingCharacters(in: .whitespaces).isEmpty {
            let word = try Self.term(line)
            words[word.lowercased()] = word
        }
        return words.values.sorted()
    }

    func command(_ arguments: [String]) throws -> [String] {
        try Self.validate(arguments)
        let previous = try words()
        var words = Dictionary(uniqueKeysWithValues: previous.map { ($0.lowercased(), $0) })
        for value in arguments.dropFirst() {
            let word = try Self.term(value)
            if arguments[0] == "add" { words[word.lowercased()] = word }
            else { words.removeValue(forKey: word.lowercased()) }
        }
        let result = words.values.sorted()
        guard result.count <= 256 else { throw TalkError("Keep vocabulary to 256 words or phrases.") }
        if result != previous || !FileManager.default.fileExists(atPath: file.path) {
            try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true,
                                                    attributes: [.posixPermissions: 0o700])
            try (result.isEmpty ? "" : result.joined(separator: "\n") + "\n").write(to: file, atomically: true, encoding: .utf8)
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
        }
        return result
    }

    func prepare() throws -> URL {
        _ = try command([])
        return file
    }
}
