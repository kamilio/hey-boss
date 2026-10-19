import Foundation
import SQLite3
import Darwin

public struct Dictation: Codable, Equatable, Identifiable {
    public let id: UUID
    public let createdAt: Date
    public let text: String
    public let audioSeconds: Double
    public let source: String

    public init(text: String, audioSeconds: Double, source: String) {
        id = UUID(); createdAt = Date()
        self.text = text; self.audioSeconds = audioSeconds; self.source = source
    }

    private init(id: UUID, createdAt: Date, text: String, audioSeconds: Double, source: String) {
        self.id = id; self.createdAt = createdAt; self.text = text
        self.audioSeconds = audioSeconds; self.source = source
    }

    public var json: [String: Any] {
        ["id": id.uuidString, "created_at": ISO8601DateFormatter().string(from: createdAt),
         "text": text, "audio_seconds": audioSeconds, "source": source]
    }

    fileprivate static func read(_ statement: OpaquePointer) throws -> Dictation {
        func string(_ column: Int32) -> String { String(cString: sqlite3_column_text(statement, column)) }
        guard let id = UUID(uuidString: string(0)) else { throw TalkError("Invalid dictation ID in history") }
        return Dictation(id: id, createdAt: Date(timeIntervalSinceReferenceDate: sqlite3_column_double(statement, 1)),
                         text: string(2), audioSeconds: sqlite3_column_double(statement, 3), source: string(4))
    }
}

/// Use from one serial queue. Each INSERT commits durably before paste is attempted.
/// SQLite pages older entries without loading or rewriting the full history.
public final class HistoryStore {
    private var database: OpaquePointer?
    private let transient = unsafeBitCast(-1, to: sqlite3_destructor_type.self)

    public init(directory: URL) throws {
        let files = FileManager.default
        try files.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        var info = stat()
        guard lstat(directory.path, &info) == 0, info.st_uid == getuid(), info.st_mode & S_IFMT == S_IFDIR,
              chmod(directory.path, 0o700) == 0 else { throw TalkError("Unsafe history directory") }
        let file = directory.appendingPathComponent("history.sqlite3")
        let fd = open(file.path, O_CREAT | O_RDWR | O_NOFOLLOW | O_CLOEXEC, 0o600)
        guard fd >= 0 else { throw TalkError("Cannot open dictation history") }
        let safe = fstat(fd, &info) == 0 && info.st_uid == getuid() && info.st_mode & S_IFMT == S_IFREG
        let secured = fchmod(fd, 0o600) == 0
        close(fd)
        guard safe, secured else { throw TalkError("Unsafe history file") }
        do {
            guard sqlite3_open_v2(file.path, &database, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_NOMUTEX, nil) == SQLITE_OK else { throw failure() }
            sqlite3_busy_timeout(database, 2_000)
            try execute("PRAGMA synchronous = FULL")
            try execute("CREATE TABLE IF NOT EXISTS dictations (id TEXT NOT NULL UNIQUE, created_at REAL NOT NULL, text TEXT NOT NULL, audio_seconds REAL NOT NULL, source TEXT NOT NULL)")
        } catch {
            sqlite3_close(database); database = nil
            throw error
        }
    }

    public func append(_ entry: Dictation) throws {
        let statement = try prepare("INSERT INTO dictations (id, created_at, text, audio_seconds, source) VALUES (?, ?, ?, ?, ?)")
        defer { sqlite3_finalize(statement) }
        sqlite3_bind_text(statement, 1, entry.id.uuidString, -1, transient)
        sqlite3_bind_double(statement, 2, entry.createdAt.timeIntervalSinceReferenceDate)
        sqlite3_bind_text(statement, 3, entry.text, -1, transient)
        sqlite3_bind_double(statement, 4, entry.audioSeconds)
        sqlite3_bind_text(statement, 5, entry.source, -1, transient)
        guard sqlite3_step(statement) == SQLITE_DONE else { throw failure() }
    }

    public func recent(limit: Int = 50, before: UUID? = nil) throws -> [Dictation] {
        let statement = try prepare("SELECT id, created_at, text, audio_seconds, source FROM dictations \(before == nil ? "" : "WHERE rowid < (SELECT rowid FROM dictations WHERE id = ?)") ORDER BY rowid DESC LIMIT ?")
        defer { sqlite3_finalize(statement) }
        if let before { sqlite3_bind_text(statement, 1, before.uuidString, -1, transient) }
        sqlite3_bind_int64(statement, before == nil ? 1 : 2, Int64(max(1, min(limit, 500))))
        var entries: [Dictation] = []
        while true {
            switch sqlite3_step(statement) {
            case SQLITE_ROW: entries.append(try Dictation.read(statement))
            case SQLITE_DONE: return entries
            default: throw failure()
            }
        }
    }

    private func failure() -> TalkError {
        TalkError("Could not save or read dictation history: \(database.map { String(cString: sqlite3_errmsg($0)) } ?? "database unavailable")")
    }
    private func execute(_ sql: String) throws {
        guard sqlite3_exec(database, sql, nil, nil, nil) == SQLITE_OK else { throw failure() }
    }
    private func prepare(_ sql: String) throws -> OpaquePointer {
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(database, sql, -1, &statement, nil) == SQLITE_OK, let statement else { throw failure() }
        return statement
    }
    deinit { sqlite3_close(database) }
}
