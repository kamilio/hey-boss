import AppKit
import TalkCore

struct Clip: Codable, Identifiable, Equatable {
    var id = UUID()
    var text: String
    var app: String
    var copiedAt = Date()
    var pinned = false

    var preview: String { String(text.prefix(240)) }

    var summary: [String: Any] {
        ["id": id.uuidString, "preview": preview, "characters": text.count,
         "app": app, "copied_at": ISO8601DateFormatter().string(from: copiedAt), "pinned": pinned]
    }
}

final class History: ObservableObject {
    static let directory = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/hey-clipboard")
    @Published private(set) var clips: [Clip]
    @Published private(set) var warning: String?
    private let file: URL
    private let writes = DispatchQueue(label: "hey-clipboard.history", qos: .utility)

    init(directory: URL = History.directory) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        var info = stat()
        guard lstat(directory.path, &info) == 0, info.st_uid == getuid(), info.st_mode & S_IFMT == S_IFDIR else { throw TalkError("Unsafe history directory") }
        chmod(directory.path, 0o700)
        file = directory.appendingPathComponent("history.json")
        clips = FileManager.default.fileExists(atPath: file.path) ? try JSONDecoder().decode([Clip].self, from: Data(contentsOf: file)) : []
    }

    func matching(_ query: String) -> [Clip] {
        query.isEmpty ? clips : clips.filter { $0.text.localizedStandardContains(query) || $0.app.localizedStandardContains(query) }
    }
    @discardableResult func add(_ text: String, app: String, date: Date = Date()) -> Clip? {
        guard !text.isEmpty, text.utf8.count <= 1_048_576 else { return nil }
        var clip: Clip
        if let index = clips.firstIndex(where: { $0.text == text }) { clip = clips.remove(at: index); clip.copiedAt = date }
        else { clip = Clip(text: text, app: app, copiedAt: date) }
        clips.insert(clip, at: 0)
        var unpinned = 0
        clips.removeAll { clip in
            if clip.pinned { return false }
            unpinned += 1
            return unpinned > 40
        }
        save()
        return clip
    }
    func delete(_ id: UUID) { clips.removeAll { $0.id == id }; save() }
    func clear() { clips.removeAll(); save() }
    func pin(_ id: UUID, _ value: Bool) {
        guard let index = clips.firstIndex(where: { $0.id == id }) else { return }
        clips[index].pinned = value
        save()
    }
    // Flycut saves newest first. Preserve both its recency and favorites, without overwriting newer copies.
    func importFlycut(_ data: Data) throws -> Int {
        guard let preferences = try PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
              let store = preferences["store"] as? [String: Any] else { throw TalkError("No saved Flycut history found. Quit Flycut first so it saves its entries.") }
        let recent = store["jcList"] as? [[String: Any]] ?? []
        let favorites = store["favoritesList"] as? [[String: Any]] ?? []
        var imported = 0
        for (rows, pinned) in [(favorites, true), (recent, false)] {
            for row in rows {
                guard let text = row["Contents"] as? String, !text.isEmpty else { continue }
                if let index = clips.firstIndex(where: { $0.text == text }) {
                    if pinned { clips[index].pinned = true }
                    continue
                }
                clips.append(Clip(text: text, app: row["AppLocalizedName"] as? String ?? "Flycut",
                                  copiedAt: Date(timeIntervalSince1970: row["Timestamp"] as? Double ?? 0), pinned: pinned))
                imported += 1
            }
        }
        save()
        return imported
    }
    func finishWrites() { writes.sync {} }
    private func save() {
        let snapshot = clips, file = file
        writes.async {
            do {
                let data = try JSONEncoder().encode(snapshot)
                try data.write(to: file, options: .atomic)
                chmod(file.path, 0o600)
            } catch {
                DispatchQueue.main.async { self.warning = "History could not be saved: " + error.localizedDescription }
            }
        }
    }
}

final class ClipboardMonitor {
    private let board: NSPasteboard
    private var changeCount: Int
    private var lastText: String?
    var paused = false
    private(set) var skipped = 0
    private(set) var captured = 0
    let history: History

    init(history: History, board: NSPasteboard = .general) {
        self.history = history; self.board = board
        changeCount = board.changeCount
        lastText = Self.excluded(board) ? nil : board.string(forType: .string)
    }
    static func excluded(_ board: NSPasteboard) -> Bool {
        let ignored = Set(["org.nspasteboard.TransientType", "org.nspasteboard.ConcealedType", "org.nspasteboard.AutoGeneratedType", "PasswordPboardType", "com.agilebits.onepassword"])
        let types = (board.types ?? []) + (board.pasteboardItems ?? []).flatMap(\.types)
        return types.contains { ignored.contains($0.rawValue) }
    }
    func poll(app: String) {
        let count = board.changeCount
        guard count != changeCount else { return }
        changeCount = count
        guard !paused, !Self.excluded(board) else { skipped += 1; return }
        guard let text = board.string(forType: .string), board.changeCount == count else { return }
        guard text != lastText else { return }
        lastText = text
        if history.add(text, app: app) != nil { captured += 1 }
    }
    func copy(_ clip: Clip) -> Bool {
        board.clearContents()
        guard board.setString(clip.text, forType: .string) else { return false }
        changeCount = board.changeCount; lastText = clip.text
        history.add(clip.text, app: clip.app)
        return true
    }
}
