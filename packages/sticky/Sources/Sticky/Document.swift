import Foundation

@_silgen_name("sticky_render") private func renderMarkdown(_ source: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("sticky_render_free") private func freeMarkdown(_ source: UnsafeMutablePointer<CChar>)

struct RenderedDocument: Decodable {
    let title: String
    let html: String
    init(_ source: String) throws {
        guard !source.utf8.contains(0), source.utf8.count <= 1_048_576 else {
            throw StickyError("Notes must be UTF-8 text without NUL characters, up to 1 MiB.")
        }
        guard let pointer = source.withCString({ renderMarkdown($0) }) else { throw StickyError("Could not render Markdown.") }
        defer { freeMarkdown(pointer) }
        self = try JSONDecoder().decode(Self.self, from: Data(String(cString: pointer).utf8))
    }
}

struct StickyError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}

struct Note {
    let id: String
    var markdown: String
    var title: String { (try? RenderedDocument(markdown).title) ?? "Sticky" }
}

final class NoteStore {
    let directory: URL
    init(directory: URL = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Sticky/Notes")) throws {
        self.directory = directory
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
    }
    func url(_ id: String) throws -> URL {
        guard !id.isEmpty, id.count <= 80, id.utf8.allSatisfy({ (97...122).contains($0) || (48...57).contains($0) || $0 == 45 }) else {
            throw StickyError("Note IDs use lowercase letters, numbers, and hyphens (up to 80 characters).")
        }
        return directory.appendingPathComponent(id + ".md")
    }
    func read(_ id: String) throws -> Note {
        let url = try url(id)
        let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
        guard size <= 1_048_576 else { throw StickyError("Note exceeds 1 MiB.") }
        return Note(id: id, markdown: try String(contentsOf: url, encoding: .utf8))
    }
    func all() throws -> [Note] {
        try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "md" }.sorted { $0.lastPathComponent < $1.lastPathComponent }
            .map { try read($0.deletingPathExtension().lastPathComponent) }
    }
    func save(_ note: Note) throws {
        _ = try RenderedDocument(note.markdown)
        let url = try url(note.id)
        try Data(note.markdown.utf8).write(to: url, options: .atomic)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }
    func ensureDefault() throws {
        if try all().isEmpty { try save(Note(id: "focus", markdown: "# Daily Focus\n\n")) }
    }
}
