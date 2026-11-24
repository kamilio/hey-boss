import AppKit
import CryptoKit
import ImageIO
import TalkCore

struct Clip: Codable, Identifiable, Equatable {
    var id = UUID()
    var text: String
    var app: String
    var copiedAt = Date()
    var pinned = false
    var imageType: String?
    var imageHash: String?

    var preview: String { imageType == nil ? String(text.prefix(240)) : "Image" }

    var summary: [String: Any] {
        ["id": id.uuidString, "preview": preview, "characters": text.count,
         "app": app, "copied_at": ISO8601DateFormatter().string(from: copiedAt), "pinned": pinned,
         "type": imageType ?? NSPasteboard.PasteboardType.string.rawValue]
    }
}

final class History: ObservableObject {
    static let directory = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/hey-clipboard")
    @Published private(set) var clips: [Clip]
    @Published private(set) var warning: String?
    private let file: URL
    private let writes = DispatchQueue(label: "hey-clipboard.history", qos: .utility)
    private let thumbnails = NSCache<NSUUID, NSImage>()

    init(directory: URL = History.directory) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        var info = stat()
        guard lstat(directory.path, &info) == 0, info.st_uid == getuid(), info.st_mode & S_IFMT == S_IFDIR else { throw TalkError("Unsafe history directory") }
        chmod(directory.path, 0o700)
        file = directory.appendingPathComponent("history.json")
        clips = FileManager.default.fileExists(atPath: file.path) ? try JSONDecoder().decode([Clip].self, from: Data(contentsOf: file)) : []
        thumbnails.totalCostLimit = 32 * 1024 * 1024
    }

    func matching(_ query: String) -> [Clip] {
        query.isEmpty ? clips : clips.filter { $0.text.localizedStandardContains(query) || $0.preview.localizedStandardContains(query) || $0.app.localizedStandardContains(query) }
    }
    @discardableResult func add(_ text: String, app: String, date: Date = Date()) -> Clip? {
        guard !text.isEmpty, text.utf8.count <= 1_048_576 else { return nil }
        var clip: Clip
        if let index = clips.firstIndex(where: { $0.imageType == nil && $0.text == text }) { clip = clips.remove(at: index); clip.copiedAt = date }
        else { clip = Clip(text: text, app: app, copiedAt: date) }
        insert(clip)
        return clip
    }
    @discardableResult func add(_ data: Data, type: NSPasteboard.PasteboardType, app: String) -> Clip? {
        guard [.png, .tiff].contains(type), !data.isEmpty, data.count <= 32 * 1024 * 1024 else { return nil }
        let hash = Self.imageHash(data)
        var clip: Clip
        if let index = clips.firstIndex(where: { $0.imageHash == hash && $0.imageType == type.rawValue }) {
            clip = clips.remove(at: index); clip.copiedAt = Date()
        } else {
            clip = Clip(text: "", app: app, imageType: type.rawValue, imageHash: hash)
            do {
                try data.write(to: imageURL(clip), options: .atomic)
                chmod(imageURL(clip).path, 0o600)
            } catch {
                warning = "Image could not be saved: " + error.localizedDescription
                return nil
            }
        }
        insert(clip)
        return clip
    }
    static func imageHash(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }
    func imageURL(_ clip: Clip) -> URL {
        file.deletingLastPathComponent().appendingPathComponent(clip.id.uuidString + (clip.imageType == NSPasteboard.PasteboardType.png.rawValue ? ".png" : ".tiff"))
    }
    func thumbnail(_ clip: Clip) -> NSImage? {
        guard clip.imageType != nil else { return nil }
        if let image = thumbnails.object(forKey: clip.id as NSUUID) { return image }
        guard let source = CGImageSourceCreateWithURL(imageURL(clip) as CFURL, nil),
              let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: 480
              ] as CFDictionary) else { return nil }
        let result = NSImage(cgImage: image, size: .zero)
        thumbnails.setObject(result, forKey: clip.id as NSUUID, cost: image.bytesPerRow * image.height)
        return result
    }
    private func insert(_ clip: Clip) {
        clips.insert(clip, at: 0)
        var unpinned = 0
        var removed: [Clip] = []
        clips.removeAll { clip in
            if clip.pinned { return false }
            unpinned += 1
            if unpinned > 40 { removed.append(clip); return true }
            return false
        }
        save(removing: removed)
    }
    func delete(_ id: UUID) {
        let removed = clips.filter { $0.id == id }
        clips.removeAll { $0.id == id }; save(removing: removed)
    }
    func clear() { let removed = clips; clips.removeAll(); save(removing: removed) }
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
    private func save(removing removed: [Clip] = []) {
        let snapshot = clips, file = file
        let images = removed.filter { $0.imageType != nil }.map(imageURL)
        for clip in removed { thumbnails.removeObject(forKey: clip.id as NSUUID) }
        writes.async {
            do {
                let data = try JSONEncoder().encode(snapshot)
                try data.write(to: file, options: .atomic)
                chmod(file.path, 0o600)
                for image in images where FileManager.default.fileExists(atPath: image.path) { try FileManager.default.removeItem(at: image) }
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
    private var lastImageHash: String?
    var paused = false
    private(set) var skipped = 0
    private(set) var captured = 0
    let history: History

    init(history: History, board: NSPasteboard = .general) {
        self.history = history; self.board = board
        changeCount = board.changeCount
        if !Self.excluded(board) {
            if let type = board.availableType(from: [.png, .tiff]), let data = board.data(forType: type), data.count <= 32 * 1024 * 1024 {
                lastImageHash = History.imageHash(data)
            } else { lastText = board.string(forType: .string) }
        }
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
        if let type = board.availableType(from: [.png, .tiff]), let data = board.data(forType: type) {
            guard board.changeCount == count, data.count <= 32 * 1024 * 1024 else { return }
            let hash = History.imageHash(data)
            guard hash != lastImageHash else { return }
            if history.add(data, type: type, app: app) != nil {
                lastImageHash = hash; lastText = nil; captured += 1
            }
        } else if let text = board.string(forType: .string), board.changeCount == count {
            guard text != lastText else { return }
            lastText = text; lastImageHash = nil
            if history.add(text, app: app) != nil { captured += 1 }
        }
    }
    func copy(_ clip: Clip) -> Bool {
        if let type = clip.imageType {
            guard let data = try? Data(contentsOf: history.imageURL(clip)) else { return false }
            board.clearContents()
            guard board.setData(data, forType: NSPasteboard.PasteboardType(type)) else { return false }
            history.add(data, type: NSPasteboard.PasteboardType(type), app: clip.app)
            lastText = nil; lastImageHash = clip.imageHash
        } else {
            board.clearContents()
            guard board.setString(clip.text, forType: .string) else { return false }
            history.add(clip.text, app: clip.app)
            lastText = clip.text; lastImageHash = nil
        }
        changeCount = board.changeCount
        return true
    }
}
