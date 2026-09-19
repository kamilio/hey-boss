import AppKit
import Foundation
import SQLite3
import ImageIO
import WebKit
import Darwin
import QuartzCore
import IOKit

/// Only aggregate input idle time is sampled. No keys, cursor positions or app
/// contents are recorded or sent to the hub.
final class MacPresence {
    private let lock = NSLock()
    private var locked = false
    private var sleeping = false
    private let hidService = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("IOHIDSystem"))
    private var observers: [NSObjectProtocol] = []
    private var distributed: [NSObjectProtocol] = []
    init() {
        locked = (CGSessionCopyCurrentDictionary() as? [String: Any])?["CGSSessionScreenIsLocked"] as? Bool ?? false
        let center = DistributedNotificationCenter.default()
        distributed.append(center.addObserver(forName: .init("com.apple.screenIsLocked"), object: nil, queue: .main) { [weak self] _ in self?.setLocked(true) })
        distributed.append(center.addObserver(forName: .init("com.apple.screenIsUnlocked"), object: nil, queue: .main) { [weak self] _ in self?.setLocked(false) })
        let workspace = NSWorkspace.shared.notificationCenter
        observers.append(workspace.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main) { [weak self] _ in self?.setSleeping(true) })
        observers.append(workspace.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in self?.setSleeping(false) })

    }
    private func setLocked(_ value: Bool) { lock.lock(); locked = value; lock.unlock() }
    private func setSleeping(_ value: Bool) { lock.lock(); sleeping = value; lock.unlock() }
    var snapshot: [String: Any] {
        lock.lock(); let unavailable = locked || sleeping; lock.unlock()
        let sample = hidIdleSeconds
        return ["idleSeconds": min(604800, max(0, sample ?? 0)), "idleReliable": sample != nil, "unavailable": unavailable]
    }
    var hidIdleSeconds: Double? {
        guard hidService != 0,
              let value = IORegistryEntryCreateCFProperty(hidService, "HIDIdleTime" as CFString, kCFAllocatorDefault, 0)?.takeRetainedValue() as? NSNumber else { return nil }
        let seconds = value.doubleValue / 1_000_000_000
        return seconds.isFinite && seconds >= 0 ? seconds : nil
    }
    deinit {
        for observer in observers { NSWorkspace.shared.notificationCenter.removeObserver(observer) }
        for observer in distributed { DistributedNotificationCenter.default().removeObserver(observer) }
        if hidService != 0 { IOObjectRelease(hidService) }
    }
}

@_silgen_name("launch_activate_socket")
func launchActivateSocket(_ name: UnsafePointer<CChar>, _ fds: UnsafeMutablePointer<UnsafeMutablePointer<Int32>?>, _ count: UnsafeMutablePointer<Int>) -> Int32

func onMain(_ action: @escaping () -> Void) {
    RunLoop.main.perform(inModes: [.common], block: action)
    CFRunLoopWakeUp(CFRunLoopGetMain())
}

struct IssueReference: Codable, Equatable {
    let project: String
    let number: Int64
    let host: String?
    func validate() throws {
        guard !project.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, project.utf8.count <= 8192,
              !project.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }), number > 0 else { throw StorageError(description: "Invalid issue relationship") }
        if let host { guard !host.isEmpty, host.utf8.count <= 253, host.first != "-", host.unicodeScalars.allSatisfy({ $0.isASCII && CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-@:[]").contains($0) }) else { throw StorageError(description: "Invalid issue SSH host") } }
    }
}

struct Request: Decodable {
    let command: String
    let issue: IssueReference?
    let question: String?
    let project: String?
    let title: String?
    let description: String?
    let options: [String]?
    let autoclose: Double?
    let link_url: String?
    let link_label: String?
    let task_id: String?
    let document_name: String?
    let attachment: DocumentAttachment?
    let comments_enabled: Bool?
    let sync: Bool
    let bridge_generation: String?
    let bridge_host: String?
    let source_host: String?
    let origin: LaunchOrigin?
    let severity: String?
    let icon: String?
    let icon_path: String?
}

struct Launcher: Codable {
    let pid: UInt32
    let executable: String
}

struct GitContext: Codable {
    let root: String
    let branch: String?
    let revision: String?
}

struct LaunchOrigin: Codable {
    let cwd: String
    let pid: UInt32
    let executable: String
    let git: GitContext?
    let launchers: [Launcher]
}

struct DocumentAttachment: Codable {
    let name: String
    let mime: String
    let data: String
    func validatedImage() throws -> Data {
        guard name.utf8.count <= 1024, data.utf8.count <= 6 * 1024 * 1024,
              ["image/png", "image/jpeg", "image/gif", "image/webp"].contains(mime),
              let bytes = Data(base64Encoded: data), bytes.count <= 4 * 1024 * 1024,
              let image = CGImageSourceCreateWithData(bytes as CFData, [kCGImageSourceShouldCache: false] as CFDictionary),
              let properties = CGImageSourceCopyPropertiesAtIndex(image, 0, nil) as? [CFString: Any],
              let width = properties[kCGImagePropertyPixelWidth] as? NSNumber, let height = properties[kCGImagePropertyPixelHeight] as? NSNumber,
              width.int64Value > 0, height.int64Value > 0, width.int64Value <= 16384, height.int64Value <= 16384,
              CGImageSourceGetCount(image) > 0, CGImageSourceGetCount(image) <= 200,
              width.int64Value * height.int64Value <= 16_777_216 / Int64(CGImageSourceGetCount(image)) else { throw StorageError(description: "Review image is invalid or exceeds image limits") }
        return bytes
    }
}

struct DocumentSelection: Codable {
    let line_start: Int
    let line_end: Int
    let source_text: String
    var json: [String: Any] { ["line_start": line_start, "line_end": line_end, "source_text": source_text] }
}
struct DocumentComment: Codable {
    let id: String
    let text: String
    let quote: String?
    let created_at: Double
    var selection: DocumentSelection? = nil
    var json: [String: Any] {
        var result: [String: Any] = ["id": id, "text": text, "created_at": created_at]
        if let quote { result["quote"] = quote }
        if let selection { result["selection"] = selection.json }
        return result
    }
}

struct Record: Codable {
    let taskID: String
    let kind: String
    let question: String
    var project: String?
    var title: String?
    let description: String
    let options: [String]
    let autoclose: Double?
    let linkURL: String?
    let linkLabel: String?
    let createdAt: Double
    var presentedAt: Double?
    var expiresAt: Double?
    var status: String
    var result: String?
    var origin: LaunchOrigin?
    var documentName: String?
    var attachment: DocumentAttachment?
    var commentsEnabled: Bool?
    var comments: [DocumentComment]?
    var sourceKnown: Bool?
    var sourceHost: String?
    var completedAt: Double?
    var severity: String?
    var icon: String?
    var iconData: Data?
    var bannerHidden: Bool?
    var issue: IssueReference?

    var isLocalSource: Bool { sourceHost == "This Mac" }
    var sourceLabel: String {
        guard let host = sourceHost else { return sourceKnown == true ? "This Mac" : "Source unavailable" }
        let clean = String(host.unicodeScalars.filter { !CharacterSet.controlCharacters.contains($0) }.map(String.init).joined().prefix(80))
        return clean.isEmpty ? "Server" : clean
    }
    var heading: String { "\(project ?? "Notifications") · \(title ?? "Untitled")" }

    var response: [String: Any] {
        var response: [String: Any] = ["task_id": taskID, "status": status]
        if let issue, let data = try? JSONEncoder().encode(issue), let json = try? JSONSerialization.jsonObject(with: data) { response["issue"] = json }
        if let result { response["result"] = result }
        if let documentName { response["document_name"] = documentName }
        if commentsEnabled == true {
            response["review_status"] = status == "pending" ? "open" : (status == "cancelled" ? "cancelled" : "finished")
            if let comments, !comments.isEmpty { response["comments"] = comments.map(\.json) }
        }
        return response
    }
}

func readRequest(_ fd: Int32) throws -> Data {
    let started = DispatchTime.now().uptimeNanoseconds
    let budget: UInt64 = 10_000_000_000
    var data = Data()
    var buffer = [UInt8](repeating: 0, count: 8192)
    while true {
        let elapsed = DispatchTime.now().uptimeNanoseconds &- started
        guard elapsed < budget else { throw StorageError(description: "Request timed out") }
        let remainingMilliseconds = Int32((budget - elapsed) / 1_000_000 + 1)
        var descriptor = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
        let ready = poll(&descriptor, 1, remainingMilliseconds)
        if ready < 0 && errno == EINTR { continue }
        guard ready > 0 else { throw StorageError(description: "Request timed out or disconnected") }
        let count = Darwin.read(fd, &buffer, buffer.count)
        if count == 0 { return data }
        if count < 0 && errno == EINTR { continue }
        guard count > 0 else { throw StorageError(description: "Request read failed") }
        guard data.count + count <= 8 * 1024 * 1024 else { throw StorageError(description: "Request exceeds size limit") }
        data.append(contentsOf: buffer.prefix(count))
    }
}

final class Reply {
    let fd: Int32
    var isConnected: Bool {
        var descriptor = pollfd(fd: fd, events: Int16(POLLOUT), revents: 0)
        return poll(&descriptor, 1, 0) >= 0 && descriptor.revents & Int16(POLLHUP | POLLERR | POLLNVAL) == 0
    }
    let channel: DispatchIO
    init(_ fd: Int32) {
        self.fd = fd
        channel = DispatchIO(type: .stream, fileDescriptor: fd, queue: .global(qos: .userInitiated)) { _ in Darwin.close(fd) }
    }
    func send(_ response: [String: Any]) {
        guard let bytes = try? JSONSerialization.data(withJSONObject: response) else { channel.close(); return }
        let data = bytes.withUnsafeBytes { DispatchData(bytes: $0) }
        channel.write(offset: 0, data: data, queue: .global(qos: .userInitiated)) { [self] done, _, _ in
            if done { channel.close() }
        }
        // Start this deadline only when answering; questions can await user input.
        DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 10) { [weak self] in
            self?.channel.close(flags: .stop)
        }
    }
}

struct StorageError: Error, CustomStringConvertible {
    let description: String
}

func readScannerOutput(_ handle: FileHandle) throws -> Data {
    var data = Data()
    while let chunk = try handle.read(upToCount: 64 * 1024), !chunk.isEmpty {
        guard data.count + chunk.count <= 8 * 1024 * 1024 else {
            throw StorageError(description: "Scanner output exceeds 8 MiB; local data may be stale.")
        }
        data.append(chunk)
    }
    return data
}

func reportFailure(_ error: Error) {
    fputs("hey-boss: \(error)\n", stderr)
}

final class Database {
    let db: OpaquePointer
    init(_ path: String) throws {
        var pointer: OpaquePointer?
        let code = sqlite3_open(path, &pointer)
        guard code == SQLITE_OK, let opened = pointer else {
            if let pointer { sqlite3_close(pointer) }
            throw StorageError(description: "Cannot open notification history (SQLite \(code))")
        }
        db = opened
        sqlite3_busy_timeout(db, 3000)
        try execute("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS dialogs (id TEXT PRIMARY KEY, status TEXT NOT NULL, body TEXT NOT NULL); CREATE INDEX IF NOT EXISTS pending ON dialogs(status);")
        try execute("UPDATE dialogs SET body=json_set(body, '$.title', json_extract(body, '$.description')) WHERE json_valid(body) AND json_extract(body, '$.kind')='update' AND json_extract(body, '$.title') IS NULL;")
        try execute("CREATE VIEW IF NOT EXISTS notifications AS SELECT id, json_extract(body, '$.project') AS project, json_extract(body, '$.title') AS title, json_extract(body, '$.kind') AS kind, json_extract(body, '$.question') AS message, json_extract(body, '$.description') AS summary, status, json_extract(body, '$.createdAt') AS created_at, json_extract(body, '$.presentedAt') AS presented_at, json_extract(body, '$.completedAt') AS completed_at FROM dialogs;")
    }
    deinit { sqlite3_close(db) }
    func failure() -> StorageError { StorageError(description: String(cString: sqlite3_errmsg(db))) }
    func execute(_ sql: String) throws {
        guard sqlite3_exec(db, sql, nil, nil, nil) == SQLITE_OK else { throw failure() }
    }
    func statement(_ sql: String) throws -> OpaquePointer {
        var pointer: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &pointer, nil) == SQLITE_OK, let pointer else { throw failure() }
        return pointer
    }
    func save(_ row: Record) throws {
        let body = String(decoding: try JSONEncoder().encode(row), as: UTF8.self)
        let stmt = try statement("INSERT INTO dialogs VALUES (?, ?, ?) ON CONFLICT(id) DO UPDATE SET status=excluded.status, body=excluded.body")
        defer { sqlite3_finalize(stmt) }
        let transient = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
        for (index, value) in [row.taskID, row.status, body].enumerated() {
            guard sqlite3_bind_text(stmt, Int32(index + 1), value, -1, transient) == SQLITE_OK else { throw failure() }
        }
        guard sqlite3_step(stmt) == SQLITE_DONE else { throw failure() }
    }
    func get(_ id: String) throws -> Record {
        let stmt = try statement("SELECT body FROM dialogs WHERE id=?")
        defer { sqlite3_finalize(stmt) }
        let transient = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
        guard sqlite3_bind_text(stmt, 1, id, -1, transient) == SQLITE_OK else { throw failure() }
        let code = sqlite3_step(stmt)
        guard code == SQLITE_ROW else {
            if code == SQLITE_DONE { throw StorageError(description: "Unknown task ID") }
            throw failure()
        }
        guard let body = sqlite3_column_text(stmt, 0) else { throw StorageError(description: "Missing history record") }
        return try JSONDecoder().decode(Record.self, from: Data(String(cString: body).utf8))
    }
    func pending() throws -> [Record] {
        let stmt = try statement("SELECT body FROM dialogs WHERE status='pending' ORDER BY rowid")
        defer { sqlite3_finalize(stmt) }
        var rows: [Record] = []
        while true {
            let code = sqlite3_step(stmt)
            if code == SQLITE_DONE { return rows }
            guard code == SQLITE_ROW else { throw failure() }
            guard let body = sqlite3_column_text(stmt, 0) else { continue }
            do { rows.append(try JSONDecoder().decode(Record.self, from: Data(String(cString: body).utf8))) }
            catch { reportFailure(StorageError(description: "Skipped damaged history record: \(error)")) }
        }
    }
    func inboxRows() throws -> [[String: Any]] {
        // Project list metadata and bounded 128px icon snapshots, never document/attachment bodies.
        let stmt = try statement("SELECT json_object('taskID',id,'kind',json_extract(body,'$.kind'),'title',substr(coalesce(json_extract(body,'$.title'),json_extract(body,'$.question')),1,256),'project',json_extract(body,'$.project'),'summary',substr(CASE WHEN json_extract(body,'$.kind')='alert' THEN json_extract(body,'$.question') ELSE json_extract(body,'$.description') END,1,500),'createdAt',json_extract(body,'$.createdAt'),'completedAt',json_extract(body,'$.completedAt'),'sourceHost',json_extract(body,'$.sourceHost'),'severity',json_extract(body,'$.severity'),'icon',json_extract(body,'$.icon'),'iconData',json_extract(body,'$.iconData'),'commentsEnabled',json_extract(body,'$.commentsEnabled'),'issue',json_extract(body,'$.issue'),'status',status) FROM dialogs WHERE json_valid(body) ORDER BY coalesce(json_extract(body,'$.createdAt'),0) DESC,id")
        defer { sqlite3_finalize(stmt) }
        var rows: [[String: Any]] = []
        while true {
            let code = sqlite3_step(stmt)
            if code == SQLITE_DONE { return rows }
            guard code == SQLITE_ROW else { throw failure() }
            if let raw = sqlite3_column_text(stmt,0), let row = try JSONSerialization.jsonObject(with: Data(String(cString:raw).utf8)) as? [String:Any] { rows.append(row) }
        }
    }
    func transaction(_ action: () throws -> Void) throws {
        try execute("BEGIN IMMEDIATE")
        do { try action(); try execute("COMMIT") }
        catch { try? execute("ROLLBACK"); throw error }
    }
}

final class MobileHub {
    struct Configuration: Decodable { let url: String; let token: String }
    unowned let store: Store
    let configuration: Configuration
    var timer: DispatchSourceTimer?
    let networkQueue = DispatchQueue(label: "hey-boss.mobile-sync", qos: .utility)
    var presence: () -> [String: Any] = { ["idleSeconds": 0.0, "unavailable": false] }
    var onRouting: ([String: Any]) -> Void = { _ in }
    init(store: Store, configuration: Configuration) throws {
        guard let url = URL(string: configuration.url), url.scheme == "https" || (["127.0.0.1", "localhost"].contains(url.host ?? "") && url.scheme == "http"), configuration.token.count >= 32 else { throw StorageError(description: "Invalid mobile configuration") }
        self.store = store; self.configuration = configuration
        try store.database.execute("CREATE TABLE IF NOT EXISTS mobile_outbox(id TEXT PRIMARY KEY, revision INTEGER NOT NULL DEFAULT 0)")
        let columns = try store.database.statement("PRAGMA table_info(mobile_outbox)")
        var hasRevision = false
        while sqlite3_step(columns) == SQLITE_ROW { if let name = sqlite3_column_text(columns, 1), String(cString: name) == "revision" { hasRevision = true } }
        sqlite3_finalize(columns)
        if !hasRevision { try store.database.execute("ALTER TABLE mobile_outbox ADD COLUMN revision INTEGER NOT NULL DEFAULT 0") }
    }
    static func load(store: Store, directory: String) -> MobileHub? {
        let file = URL(fileURLWithPath: directory).appendingPathComponent("mobile.json")
        guard FileManager.default.fileExists(atPath: file.path) else { return nil }
        do { return try MobileHub(store: store, configuration: JSONDecoder().decode(Configuration.self, from: Data(contentsOf: file))) }
        catch { reportFailure(StorageError(description: "Mobile sync configuration could not be loaded: \(error)")); return nil }
    }
    func start() {
        store.queue.async {
            do { for row in try self.store.database.pending() { try self.track(row) } }
            catch { reportFailure(error) }
        }
        let timer = DispatchSource.makeTimerSource(queue: networkQueue)
        timer.schedule(deadline: .now(), repeating: 5, leeway: .milliseconds(500))
        timer.setEventHandler { [weak self] in self?.sync() }
        timer.resume(); self.timer = timer
    }
    func track(_ row: Record) throws { try store.database.execute("INSERT INTO mobile_outbox(id, revision) VALUES('\(row.taskID.replacingOccurrences(of: "'", with: "''"))', 1) ON CONFLICT(id) DO UPDATE SET revision=revision+1") }
    func call(_ path: String, method: String = "GET", body: [String: Any]? = nil) throws -> (Int, [String: Any]) {
        guard let base = URL(string: configuration.url), let url = URL(string: path, relativeTo: base)?.absoluteURL, url.host == base.host else { throw StorageError(description: "Invalid mobile endpoint") }
        var request = URLRequest(url: url); request.httpMethod = method; request.timeoutInterval = 2
        request.setValue("Bearer " + configuration.token, forHTTPHeaderField: "Authorization")
        if let body { request.httpBody = try JSONSerialization.data(withJSONObject: body); request.setValue("application/json", forHTTPHeaderField: "Content-Type") }
        let completed = DispatchSemaphore(value: 0); let lock = NSLock()
        var response: (Int, [String: Any])?; var failure: Error?
        let task = URLSession.shared.dataTask(with: request) { data, reply, error in
            lock.lock(); defer { lock.unlock(); completed.signal() }
            if let error { failure = error; return }
            guard let data, data.count <= 2 * 1024 * 1024, let http = reply as? HTTPURLResponse, let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { failure = StorageError(description: "Invalid mobile service response"); return }
            response = (http.statusCode, json)
        }
        task.resume()
        guard completed.wait(timeout: .now() + 3) == .success else { task.cancel(); throw StorageError(description: "Mobile service unavailable. The request remains open; try again when connected.") }
        lock.lock(); defer { lock.unlock() }
        if failure != nil { throw StorageError(description: "Mobile service unavailable. The request remains open; try again when connected.") }
        guard let response else { throw StorageError(description: "No mobile response") }; return response
    }
    func publish(_ row: Record, presenceAlreadyPublished: Bool = false) throws -> [String: Any] {
        // Publish presence first so a new item cannot race an outdated away state.
        if !presenceAlreadyPublished { try publishPresence() }
        let payload: [String: Any] = ["taskID":row.taskID,"kind":row.kind,"title":String((row.title ?? "Update").prefix(256)),"project":String((row.project ?? "Workspace").prefix(256)),"question":row.question,"description":row.description,"options":row.options,"sourceHost":row.sourceLabel,"severity":row.severity ?? "info","createdAt":row.createdAt,"linkURL":row.linkURL as Any? ?? NSNull(),"linkLabel":row.linkLabel as Any? ?? NSNull(),"commentsEnabled":row.commentsEnabled ?? false,"issue":row.issue.flatMap { try? JSONSerialization.jsonObject(with: JSONEncoder().encode($0)) } ?? NSNull()]
        let (status, json) = try call("/api/bridge/tasks/" + row.taskID, method: "PUT", body: payload)
        guard status == 200, let task = json["task"] as? [String: Any] else { throw StorageError(description: json["error"] as? String ?? "Mobile service rejected this request") }
        return task
    }
    func resolve(_ row: Record, result: String?, cancel: Bool, presenceAlreadyPublished: Bool = false) throws -> [String: Any] {
        let path = "/api/bridge/tasks/" + row.taskID + "/resolve"
        let answer: [String: Any] = ["result": result as Any? ?? NSNull(), "cancel": cancel]
        // Already-published items need one round trip, not presence + republish + resolve.
        var (status, json) = try call(path, method: "POST", body: answer)
        if status == 404 {
            let task = try publish(row, presenceAlreadyPublished: presenceAlreadyPublished)
            if task["status"] as? String != "pending" { return task }
            (status, json) = try call(path, method: "POST", body: answer)
        }
        guard [200,409].contains(status), let resolved = json["task"] as? [String: Any] else { throw StorageError(description: json["error"] as? String ?? "Mobile service rejected the answer") }
        return resolved
    }
    func sync() {
        do {
            try publishPresence()
            // Only short database operations share the store queue; HTTP never does.
            let batch: [(Record, Int64)] = try store.queue.sync {
                let statement = try store.database.statement("SELECT id, revision FROM mobile_outbox LIMIT 5")
                defer { sqlite3_finalize(statement) }
                var rows: [(Record, Int64)] = []
                while sqlite3_step(statement) == SQLITE_ROW {
                    if let raw = sqlite3_column_text(statement, 0) { rows.append((try store.database.get(String(cString: raw)), sqlite3_column_int64(statement, 1))) }
                }
                return rows
            }
            for (row, revision) in batch {
                if row.status == "pending" {
                    let task = try publish(row, presenceAlreadyPublished: true)
                    if task["status"] as? String != "pending" { try store.queue.sync { try store.applyMobile(task) } }
                } else { _ = try resolve(row, result: row.result, cancel: row.status == "cancelled", presenceAlreadyPublished: true) }
                // A newer enqueue while HTTP was in flight must remain durable.
                try store.queue.sync { try store.database.execute("DELETE FROM mobile_outbox WHERE id='\(row.taskID.replacingOccurrences(of: "'", with: "''"))' AND revision=\(revision)") }
            }
            let (status, json) = try call("/api/bridge/tasks")
            guard status == 200 else { return }
            for task in json["tasks"] as? [[String: Any]] ?? [] {
                try store.queue.sync { try store.applyMobile(task) }
                if let id = task["taskID"] as? String, let version = task["version"] as? Int { _ = try call("/api/bridge/tasks/" + id + "/ack", method: "POST", body: ["version":version]) }
            }
        } catch { onRouting(["macState": "unknown"]); /* Persistent outbox is retried. Foreground answers surface errors. */ }
    }
    func publishPresence() throws {
        let (status, response) = try call("/api/bridge/presence", method: "POST", body: presence())
        guard status == 200, let routing = response["notifications"] as? [String: Any] else { throw StorageError(description: "Activity status unavailable") }
        onRouting(routing)
    }
}

final class Store {
    let queue = DispatchQueue(label: "hey-boss.store", qos: .userInitiated)
    let database: Database
    var mobile: MobileHub?
    var mobileRequired = false
    var waiters: [String: [Reply]] = [:]
    var feedbackWaiters: [String: [Reply]] = [:]
    var show: (Record) -> Void = { _ in }
    var remove: (String) -> Void = { _ in }
    var removeMany: ([String]) -> Void = { _ in }
    var pendingChanged: (Int) -> Void = { _ in }
    func refreshPendingCount() { if let pending = try? database.pending() { pendingChanged(pending.count) } }
    init(_ path: String) throws { database = try Database(path) }
    func restore() {
        defer { refreshPendingCount() }
        do {
            for var row in try database.pending() {
                if let expiry = row.expiresAt, expiry <= Date().timeIntervalSince1970 { complete(row.taskID, nil) }
                else {
                    if row.bannerHidden == true { row.bannerHidden = false; try database.save(row) }
                    show(row)
                }
            }
        } catch { reportFailure(error) }
    }
    func handle(_ request: Request, _ reply: Reply) {
        do { try process(request, reply) }
        catch { reply.send(["status": "error", "error": String(describing: error)]); reportFailure(error) }
    }
    func process(_ request: Request, _ reply: Reply) throws {
        func invalid(_ message: String) -> StorageError { StorageError(description: message) }
        if request.command.hasPrefix("inbox_") { try processInbox(request, reply); return }
        if ["alert", "update", "ask"].contains(request.command) {
            guard let project = request.project, !project.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                  let title = request.title, !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                  let question = request.question else { throw invalid("Project, title and message are required") }
            if let seconds = request.autoclose, !seconds.isFinite || seconds <= 0 || seconds > 31_536_000 { throw invalid("Autoclose must be positive and at most 31536000 seconds") }
            guard (request.link_url == nil) == (request.link_label == nil) else { throw invalid("Link URL and label must be supplied together") }
            if let link = request.link_url {
                guard let url = URL(string: link), let scheme = url.scheme, ["https", "http", "file"].contains(scheme.lowercased()) else { throw invalid("Unsupported link URL") }
            }
            let options = request.options ?? []
            if request.command != "alert", request.description == nil { throw invalid("Description is required") }
            var row = Record(taskID: UUID().uuidString.lowercased(), kind: request.command == "update" ? "update" : (request.command == "alert" ? "alert" : (options.isEmpty ? "prompt" : "approval")), question: question, project: project, title: title, description: request.description ?? "", options: request.command == "ask" ? options : [], autoclose: request.autoclose, linkURL: request.link_url, linkLabel: request.link_label, createdAt: Date().timeIntervalSince1970, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: request.origin)
            try request.issue?.validate(); row.issue = request.issue
            row.documentName = request.document_name.map { String($0.prefix(256)) }
            if let attachment = request.attachment { _ = try attachment.validatedImage(); row.attachment = attachment }
            row.commentsEnabled = request.command == "update" && request.comments_enabled == true
            row.sourceKnown = request.source_host != nil
            row.sourceHost = request.source_host
            row.severity = request.severity; row.icon = request.icon
            row.iconData = request.icon_path.flatMap(snapshotIcon)
            try database.transaction { try database.save(row); try mobile?.track(row) }
            refreshPendingCount()
            if request.sync { waiters[row.taskID] = [reply] } else { reply.send(["task_id": row.taskID]) }
            show(row)
            return
        }
        guard ["status", "hide", "wait"].contains(request.command) else { throw invalid("Unknown command") }
        guard let id = request.task_id, !id.isEmpty else { throw invalid("Task ID is required") }
        let row = try database.get(id)
        switch request.command {
        case "status":
            if request.sync && row.status == "pending" {
                if row.commentsEnabled == true {
                    if row.comments?.isEmpty == false { reply.send(row.response) }
                    else { feedbackWaiters[row.taskID, default: []].append(reply) }
                } else if row.kind == "prompt" || row.kind == "approval" { waiters[row.taskID, default: []].append(reply) }
                else { throw invalid("Only questions or enabled document reviews can be waited on") }
            } else { reply.send(row.response) }
        case "hide":
            try dismissRecords([row.taskID])
            reply.send(try database.get(row.taskID).response)
        default:
            guard row.kind == "prompt" || row.kind == "approval" || row.commentsEnabled == true else { throw invalid("Only questions or enabled document reviews can be waited on") }
            if row.status != "pending" { reply.send(row.response) }
            else { waiters[row.taskID, default: []].append(reply) }
        }
    }
    func inboxReply(_ reply: Reply, task: Record? = nil, rows: [[String:Any]]? = nil, changed: Bool = false) throws {
        var value: [String:Any] = ["changed":changed]
        if let rows { value["tasks"] = rows; value["unread"] = rows.filter { $0["status"] as? String == "pending" }.count }
        if let task { value["task"] = try JSONSerialization.jsonObject(with: JSONEncoder().encode(task)) }
        let data = try JSONSerialization.data(withJSONObject:value)
        reply.send(["task_id":task?.taskID ?? "inbox", "status":"ok","result":String(decoding:data,as:UTF8.self)])
    }
    func processInbox(_ request: Request, _ reply: Reply) throws {
        if request.command == "inbox_list" { try inboxReply(reply,rows:database.inboxRows()); return }
        guard let id = request.task_id, !id.isEmpty, id.utf8.count <= 256 else { throw StorageError(description:"Task ID is required") }
        var row = try database.get(id)
        let before = try JSONEncoder().encode(row)
        switch request.command {
        case "inbox_view": break
        case "inbox_read":
            if row.status == "pending" && ["alert","update"].contains(row.kind) && row.commentsEnabled != true { try finish(id,nil) }
        case "inbox_respond":
            guard ["prompt","approval"].contains(row.kind), let answer = request.question, !answer.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty, answer.utf8.count <= 65536 else { throw StorageError(description:"A valid answer is required") }
            if row.status == "pending" { try finish(id,answer) }
        case "inbox_dismiss": try dismissRecords([id])
        case "inbox_comment": _ = try addComment(id,text:request.question ?? "",quote:request.description)
        case "inbox_finish_review":
            guard row.kind == "update", row.commentsEnabled == true else { throw StorageError(description:"This is not a document review") }
            if row.status == "pending" { try finish(id,nil) }
        case "inbox_link":
            try request.issue?.validate()
            if row.issue != request.issue {
                row.issue = request.issue
                try database.transaction { try database.save(row); try mobile?.track(row) }
            }
        case "inbox_open_link":
            guard let raw = row.linkURL, let url = URL(string:raw), ["https","http","file"].contains(url.scheme?.lowercased() ?? "") else { throw StorageError(description:"This notice has no supported destination") }
            onMain {
                let opened = NSWorkspace.shared.open(url)
                self.queue.async {
                    do {
                        guard opened else { throw StorageError(description:"Could not open the destination") }
                        if row.status == "pending", ["alert","update"].contains(row.kind), row.commentsEnabled != true { try self.finish(id,nil) }
                        try self.inboxReply(reply,task:self.database.get(id),changed:opened)
                    } catch { reply.send(["status":"error","error":String(describing:error)]) }
                }
            }
            return
        default: throw StorageError(description:"Unknown Inbox command")
        }
        let updated = try database.get(id)
        try inboxReply(reply,task:updated,changed:before != JSONEncoder().encode(updated))
    }
    func addComment(_ id: String, text: String, quote: String?, commentID: String? = nil, selection: DocumentSelection? = nil) throws -> Record {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, text.utf8.count <= 16 * 1024 else { throw StorageError(description: "Comment must contain 1–16384 bytes") }
        var row = try database.get(id)
        guard row.commentsEnabled == true, row.status == "pending" else { throw StorageError(description: "This document review is not open") }
        guard commentID != nil || (row.comments?.count ?? 0) < 200 else { throw StorageError(description: "Document review comment limit reached") }
        let quote = quote.map { String($0.prefix(2000)) }.flatMap { $0.isEmpty ? nil : $0 }
        if let commentID {
            guard let index = row.comments?.firstIndex(where: { $0.id == commentID }), let old = row.comments?[index] else { throw StorageError(description: "Comment no longer exists") }
            row.comments?[index] = DocumentComment(id: old.id, text: text, quote: quote, created_at: old.created_at, selection: selection ?? old.selection)
        } else {
            row.comments = (row.comments ?? []) + [DocumentComment(id: UUID().uuidString.lowercased(), text: text, quote: quote, created_at: Date().timeIntervalSince1970, selection: selection)]
        }
        guard try JSONEncoder().encode(row.comments).count <= 3 * 1024 * 1024 else { throw StorageError(description: "Document review comment storage limit reached") }
        try database.save(row)
        for reply in feedbackWaiters.removeValue(forKey: id) ?? [] { reply.send(row.response) }
        return row
    }
    func presented(_ id: String, _ timestamp: Double) {
        do {
            guard timestamp.isFinite else { return }
            var row = try database.get(id)
            guard row.status == "pending", row.presentedAt == nil else { return }
            row.presentedAt = timestamp
            if let seconds = row.autoclose { row.expiresAt = timestamp + seconds }
            try database.save(row)
        } catch { reportFailure(error) }
    }
    func complete(_ ids: [String]) { dismiss(ids) }
    func complete(_ id: String, _ result: String?) {
        do { try finish(id, result) } catch { reportFailure(error) }
    }
    func finish(_ id: String, _ result: String?) throws {
        defer { refreshPendingCount() }
        var row = try database.get(id)
        guard row.status == "pending" else { return }
        if row.kind == "approval", result == nil || !row.options.contains(result ?? "") { throw StorageError(description: "Invalid approval answer") }
        if row.kind == "prompt", result == nil { throw StorageError(description: "Missing prompt answer") }
        if mobileRequired && mobile == nil { throw StorageError(description: "Mobile configuration is invalid. Fix mobile.json before answering; the request remains open.") }
        if let mobile { try applyMobile(mobile.resolve(row, result: result, cancel: false)); return }
        row.status = "ok"; row.completedAt = Date().timeIntervalSince1970; row.result = result
        try database.save(row)
        for reply in (waiters.removeValue(forKey: id) ?? []) + (feedbackWaiters.removeValue(forKey: id) ?? []) { reply.send(row.response) }
        remove(id)
    }
    func dismiss(_ ids: [String]) {
        do { try dismissRecords(ids) } catch { reportFailure(error) }
    }
    func applyMobile(_ task: [String: Any]) throws {
        defer { refreshPendingCount() }
        guard let id = task["taskID"] as? String, let status = task["status"] as? String, ["ok", "cancelled"].contains(status) else { throw StorageError(description: "Invalid mobile outcome") }
        var row = try database.get(id)
        if row.status == "pending" {
            row.status = status; row.result = task["result"] as? String; row.completedAt = Date().timeIntervalSince1970
            try database.save(row)
            for reply in (waiters.removeValue(forKey: id) ?? []) + (feedbackWaiters.removeValue(forKey: id) ?? []) { reply.send(row.response) }
            remove(id)
        }
    }
    func dismissRecords(_ ids: [String]) throws {
        defer { refreshPendingCount() }
        if mobileRequired && mobile == nil { throw StorageError(description: "Mobile configuration is invalid; requests remain open.") }
        if let mobile {
            for id in Set(ids) { let row = try database.get(id); if row.status == "pending" { try applyMobile(mobile.resolve(row, result: nil, cancel: true)) } }
            return
        }

            var changed: [Record] = []
            try database.transaction {
                for id in Set(ids) {
                    var row = try database.get(id)
                    guard row.status == "pending" else { continue }
                    row.status = row.kind == "prompt" || row.kind == "approval" || row.commentsEnabled == true ? "cancelled" : "ok"
                    row.completedAt = Date().timeIntervalSince1970
                    try database.save(row); changed.append(row)
                }
            }
            for row in changed {
                for reply in (waiters.removeValue(forKey: row.taskID) ?? []) + (feedbackWaiters.removeValue(forKey: row.taskID) ?? []) { reply.send(row.response) }
            }
            removeMany(ids)
    }
}

enum Severity: String {
    case neutral, info, success, warning, error

    var color: NSColor {
        switch self {
        case .neutral: return .secondaryLabelColor
        case .info: return .systemBlue
        case .success: return .systemGreen
        case .warning: return .systemOrange
        case .error: return .systemRed
        }
    }
    var textColor: NSColor {
        NSColor(name: nil) { appearance in
            if appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua { return self.color }
            switch self {
            case .neutral: return .secondaryLabelColor
            case .info: return NSColor(srgbRed: 0.02, green: 0.36, blue: 0.73, alpha: 1)
            case .success: return NSColor(srgbRed: 0.12, green: 0.43, blue: 0.22, alpha: 1)
            case .warning: return NSColor(srgbRed: 0.60, green: 0.32, blue: 0.02, alpha: 1)
            case .error: return NSColor(srgbRed: 0.78, green: 0.13, blue: 0.17, alpha: 1)
            }
        }
    }
    var label: String {
        switch self {
        case .neutral: return "Update"
        case .info: return "Information"
        case .success: return "Success"
        case .warning: return "Warning"
        case .error: return "Error"
        }
    }
    var symbol: String {
        switch self {
        case .neutral: return "bell"
        case .info: return "info.circle.fill"
        case .success: return "checkmark.circle.fill"
        case .warning: return "exclamationmark.triangle.fill"
        case .error: return "xmark.octagon.fill"
        }
    }
    var priority: Int {
        switch self {
        case .neutral: return 0
        case .info: return 1
        case .success: return 2
        case .warning: return 3
        case .error: return 4
        }
    }
}

let iconSymbols = [
    "info": "info.circle.fill", "success": "checkmark.circle.fill",
    "warning": "exclamationmark.triangle.fill", "error": "xmark.octagon.fill",
    "build": "hammer.fill", "code": "chevron.left.forwardslash.chevron.right",
    "test": "checklist", "review": "text.magnifyingglass", "deploy": "shippingbox.fill",
    "docs": "doc.text.fill", "folder": "folder.fill", "bell": "bell.fill",
    "question": "questionmark.bubble.fill"
]

extension Record {
    var visualSeverity: Severity { Severity(rawValue: severity ?? "neutral") ?? .neutral }
    var visualLabel: String {
        if visualSeverity != .neutral { return visualSeverity.label }
        return kind == "update" ? "Update" : (kind == "alert" ? "Notification" : "Question")
    }
    var defaultSymbol: String {
        if visualSeverity != .neutral { return visualSeverity.symbol }
        return kind == "update" ? "doc.text.fill" : (kind == "alert" ? "bell.fill" : "questionmark.bubble.fill")
    }
}

// Read raster dimensions without decoding full-resolution pixels. A tiny compressed
// file can otherwise expand to hundreds of megabytes on the app's main thread.
func boundedIconImage(_ data: Data) -> NSImage? {
    // Some ImageIO sources recognize PDF but expose no raster properties.
    // Handle vector pages before trying the raster thumbnail path.
    if data.starts(with: Data("%PDF-".utf8)) {
        guard let page = NSPDFImageRep(data: data), page.pageCount > 0 else { return nil }
        page.currentPage = 0
        let image = NSImage(size: page.size)
        image.addRepresentation(page)
        return image
    }
    if let source = CGImageSourceCreateWithData(data as CFData, [kCGImageSourceShouldCache: false] as CFDictionary) {
        guard let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
              let width = properties[kCGImagePropertyPixelWidth] as? NSNumber,
              let height = properties[kCGImagePropertyPixelHeight] as? NSNumber else { return nil }
        let w = width.doubleValue, h = height.doubleValue
        guard w.isFinite, h.isFinite, w > 0, h > 0, w <= 16384, h <= 16384, w * h <= 16_777_216 else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: 128,
            kCGImageSourceShouldCacheImmediately: true
        ]
        guard let thumbnail = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
        return NSImage(cgImage: thumbnail, size: .zero)
    }
    return nil
}

// Store a small, self-contained image in history so caller files can be moved or deleted.
func snapshotIcon(_ path: String) -> Data? {
    guard path.hasPrefix("/"),
          let attributes = try? FileManager.default.attributesOfItem(atPath: path),
          attributes[.type] as? FileAttributeType == .typeRegular,
          let size = attributes[.size] as? NSNumber, size.intValue <= 4 * 1024 * 1024,
          let data = try? Data(contentsOf: URL(fileURLWithPath: path)), data.count <= 4 * 1024 * 1024,
          let image = boundedIconImage(data),
          image.size.width > 0, image.size.height > 0,
          image.size.width <= 16384, image.size.height <= 16384,
          let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 128, pixelsHigh: 128, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0),
          let context = NSGraphicsContext(bitmapImageRep: bitmap) else { return nil }
    let scale = min(128 / image.size.width, 128 / image.size.height)
    let fitted = NSSize(width: image.size.width * scale, height: image.size.height * scale)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    image.draw(in: NSRect(x: (128 - fitted.width) / 2, y: (128 - fitted.height) / 2, width: fitted.width, height: fitted.height), from: .zero, operation: .copy, fraction: 1)
    NSGraphicsContext.restoreGraphicsState()
    return bitmap.representation(using: .png, properties: [:])
}

final class IconBadge: NSView {
    let imageView = NSImageView()
    let row: Record
    let usesCustomImage: Bool
    init(_ row: Record, frame: NSRect) {
        self.row = row
        let custom = row.iconData.flatMap(NSImage.init(data:))
        usesCustomImage = custom != nil
        super.init(frame: frame)
        let symbol = row.icon.map { iconSymbols[$0] ?? $0 } ?? row.defaultSymbol
        imageView.image = custom ?? NSImage(systemSymbolName: symbol, accessibilityDescription: nil) ?? NSImage(systemSymbolName: row.defaultSymbol, accessibilityDescription: nil)
        imageView.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: frame.width * 0.66, weight: .medium)
        imageView.contentTintColor = custom == nil ? (row.visualSeverity == .neutral ? .labelColor : row.visualSeverity.textColor) : nil
        imageView.imageScaling = .scaleProportionallyUpOrDown
        imageView.frame = bounds.insetBy(dx: frame.width * 0.10, dy: frame.height * 0.10)
        imageView.autoresizingMask = [.width, .height]
        addSubview(imageView)
        if row.visualSeverity != .neutral && (custom != nil || symbol != row.visualSeverity.symbol) {
            let markSize = max(11, min(15, frame.width * 0.42))
            let mark = StatusMark(row.visualSeverity, frame: NSRect(x: frame.width - markSize + 1, y: 0, width: markSize, height: markSize))
            addSubview(mark)
        }
        setAccessibilityElement(true)
        setAccessibilityRole(.image)
        setAccessibilityLabel(row.visualLabel)
        toolTip = row.visualLabel
    }
    required init?(coder: NSCoder) { return nil }
    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsDisplay = true
    }
}

final class StatusMark: NSView {
    init(_ severity: Severity, frame: NSRect) {
        super.init(frame: frame)
        let image = NSImageView(frame: bounds.insetBy(dx: 1.5, dy: 1.5))
        image.image = NSImage(systemSymbolName: severity.symbol, accessibilityDescription: nil)
        image.symbolConfiguration = .init(pointSize: 12, weight: .semibold)
        image.contentTintColor = severity.textColor
        image.imageScaling = .scaleProportionallyUpOrDown
        addSubview(image)
    }
    required init?(coder: NSCoder) { return nil }
    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        NSBezierPath(ovalIn: bounds).fill()
    }
}

func severityLabel(_ row: Record, frame: NSRect) -> PlainTextField {
    let label = PlainTextField(labelWithString: row.visualLabel)
    label.font = .systemFont(ofSize: 10, weight: .semibold)
    label.textColor = row.visualSeverity.textColor
    label.frame = frame
    label.setAccessibilityLabel(row.visualLabel)
    return label
}

func markdownLinkURL(_ link: URL) -> URL? {
    if let scheme = link.scheme {
        return ["https", "http", "file"].contains(scheme.lowercased()) ? link : nil
    }
    // Agent reports commonly link directly to absolute workspace paths.
    // Relative links have no document base to resolve against.
    guard link.host == nil, link.path.hasPrefix("/") else { return nil }
    return URL(fileURLWithPath: link.path)
}

func markdown(_ source: String, size: CGFloat, color: NSColor) -> NSAttributedString {
    let output = NSMutableAttributedString(string: "")
    let lines = source.components(separatedBy: "\n")
    var fenced = false
    for (index, original) in lines.enumerated() {
        if original.hasPrefix("```") { fenced.toggle(); continue }
        var line = original
        var font = NSFont.systemFont(ofSize: size)
        let hashes = line.prefix(while: { $0 == "#" }).count
        if !fenced && hashes > 0 && hashes <= 6 && line.dropFirst(hashes).hasPrefix(" ") {
            line = String(line.dropFirst(hashes + 1))
            font = .systemFont(ofSize: max(size + 1, 24 - CGFloat(hashes) * 2), weight: .semibold)
        } else if !fenced && (line.hasPrefix("- ") || line.hasPrefix("* ") || line.hasPrefix("+ ")) {
            line = "•  " + line.dropFirst(2)
        } else if !fenced && line.hasPrefix("> ") {
            line = "│  " + line.dropFirst(2)
        }
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineSpacing = 3
        paragraph.paragraphSpacing = 3
        if fenced {
            output.append(NSAttributedString(string: line, attributes: [.font: NSFont.monospacedSystemFont(ofSize: size - 1, weight: .regular), .foregroundColor: color, .paragraphStyle: paragraph]))
        } else {
            let parsed = (try? AttributedString(markdown: line, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(line)
            for run in parsed.runs {
                let text = String(parsed[run.range].characters)
                var runFont = font
                if let intent = run.inlinePresentationIntent {
                    if intent.contains(.stronglyEmphasized) { runFont = .systemFont(ofSize: font.pointSize, weight: .bold) }
                    if intent.contains(.emphasized) { runFont = NSFontManager.shared.convert(runFont, toHaveTrait: .italicFontMask) }
                    if intent.contains(.code) { runFont = .monospacedSystemFont(ofSize: size - 1, weight: .regular) }
                }
                var attributes: [NSAttributedString.Key: Any] = [.font: runFont, .foregroundColor: color, .paragraphStyle: paragraph]
                if let rawLink = run.link, let link = markdownLinkURL(rawLink) {
                    attributes[.link] = link
                    attributes[.foregroundColor] = NSColor.linkColor
                    attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue
                }
                output.append(NSAttributedString(string: text, attributes: attributes))
            }
        }
        if index < lines.count - 1 { output.append(NSAttributedString(string: "\n", attributes: [.font: font, .paragraphStyle: paragraph])) }
    }
    return output
}

class PlainTextField: NSTextField {
    override var allowsVibrancy: Bool { false }
}

func secondaryTextColor() -> NSColor { .secondaryLabelColor }

func label(_ text: String, width: CGFloat, size: CGFloat, color: NSColor) -> NSTextField {
    let field = PlainTextField(wrappingLabelWithString: "")
    field.font = .systemFont(ofSize: size)
    field.attributedStringValue = markdown(text, size: size, color: color)
    field.isSelectable = true
    field.allowsEditingTextAttributes = true
    field.maximumNumberOfLines = 0
    field.frame = NSRect(x: 0, y: 0, width: width, height: ceil(field.sizeThatFits(NSSize(width: width, height: 100000)).height))
    return field
}

final class Panel: NSPanel, NSWindowDelegate {
    var dismissWindow: (() -> Void)?
    override func performClose(_ sender: Any?) {
        if let dismissWindow { dismissWindow() } else { super.performClose(sender) }
    }
    override func cancelOperation(_ sender: Any?) { performClose(sender) }
    var dragAnchor: NSPoint?
    var placing = false
    var dragStarted = false
    func windowDidMove(_ notification: Notification) {
        if dragStarted && !placing { rememberDrag() }
    }
    var visibleArea: NSRect {
        if dragAnchor != nil { return screen!.visibleFrame }
        return NSScreen.main!.visibleFrame
    }
    func rememberDrag() {
        dragAnchor = NSPoint(x: frame.minX, y: frame.maxY)
    }
    func place(_ proposed: NSRect, display: Bool, animated: Bool = false) {
        var target = proposed
        if let anchor = dragAnchor {
            let area = visibleArea
            target.origin = NSPoint(
                x: min(max(anchor.x, area.minX), area.maxX - target.width),
                y: min(max(anchor.y - target.height, area.minY), area.maxY - target.height)
            )
        }
        placing = true
        if animated {
            animator().setFrame(target, display: display)
        } else { setFrame(target, display: display) }
        placing = false
    }
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
    override func sendEvent(_ event: NSEvent) {
        if event.type == .keyDown && event.isARepeat && [36, 76].contains(event.keyCode) { return }
        if event.type == .keyDown && event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command {
            if event.charactersIgnoringModifiers?.lowercased() == "w" { performClose(nil); return }
            let selectors = ["a": "selectAll:", "c": "copy:", "v": "paste:", "x": "cut:", "z": "undo:"]
            if let characters = event.charactersIgnoringModifiers, let selector = selectors[characters] {
                firstResponder?.tryToPerform(Selector(selector), with: nil)
                return
            }
        }
        super.sendEvent(event)
    }
}

final class DragHeader: PlainTextField {
    var dragStart: (mouse: NSPoint, topLeft: NSPoint)?
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }
    override init(frame: NSRect) {
        super.init(frame: frame)
        let pan = NSPanGestureRecognizer(target: self, action: #selector(drag(_:)))
        pan.delaysPrimaryMouseButtonEvents = true
        addGestureRecognizer(pan)
    }
    required init?(coder: NSCoder) { return nil }
    @objc func drag(_ gesture: NSPanGestureRecognizer) {
        if gesture.state == .began {
            let panel = window as! Panel
            let mouse = NSEvent.mouseLocation
            let translation = gesture.translation(in: nil)
            panel.dragStarted = true
            dragStart = (NSPoint(x: mouse.x - translation.x, y: mouse.y - translation.y), NSPoint(x: panel.frame.minX, y: panel.frame.maxY))
        }
        if let dragStart, let panel = window as? Panel, [.began, .changed, .ended].contains(gesture.state) {
            let mouse = NSEvent.mouseLocation
            panel.setFrameOrigin(NSPoint(
                x: dragStart.topLeft.x + mouse.x - dragStart.mouse.x,
                y: dragStart.topLeft.y + mouse.y - dragStart.mouse.y - panel.frame.height
            ))
        }
        if [.ended, .cancelled, .failed].contains(gesture.state) { dragStart = nil }
    }
    override func resetCursorRects() {
        addCursorRect(bounds, cursor: .openHand)
    }
}

enum ButtonStyle { case primary, secondary, quiet, link }

class ActionButton: NSButton {
    var invoke: () -> Void = {}
    let style: ButtonStyle
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    init(_ title: String, frame: NSRect, style: ButtonStyle, action: @escaping () -> Void) {
        self.style = style
        super.init(frame: frame)
        self.title = title
        invoke = action
        target = self
        self.action = #selector(activate)
        bezelStyle = .push
        borderShape = .capsule
        controlSize = style == .quiet || style == .link ? .regular : .extraLarge
        font = .systemFont(ofSize: 13, weight: .medium)
        imageHugsTitle = true
        switch style {
        case .primary:
            tintProminence = .primary
            bezelColor = .controlAccentColor
        case .secondary:
            tintProminence = .none
        case .quiet:
            bezelStyle = .accessoryBarAction
            borderShape = title.isEmpty ? .circle : .capsule
            showsBorderOnlyWhileMouseInside = true
            tintProminence = .none
            contentTintColor = .secondaryLabelColor
        case .link:
            bezelStyle = .accessoryBarAction
            borderShape = .roundedRectangle
            showsBorderOnlyWhileMouseInside = true
            tintProminence = .none
            contentTintColor = .linkColor
        }
        if style == .primary || style == .secondary {
            let height = intrinsicContentSize.height
            self.frame = NSRect(x: frame.minX, y: frame.midY - height / 2, width: frame.width, height: height)
        }
    }
    required init?(coder: NSCoder) { return nil }
    override func resetCursorRects() {
        super.resetCursorRects()
        if style == .link { addCursorRect(bounds, cursor: .pointingHand) }
    }
    @objc func activate() { invoke() }
}

func launcherChain(_ launchers: [Launcher]) -> String {
    var names: [String] = []
    for launcher in launchers.reversed() {
        let name = URL(fileURLWithPath: launcher.executable).lastPathComponent
        let lower = name.lowercased()
        if ["node", "nodejs", "zsh", "bash", "sh", "dash", "fish", "login", "env", "ruby", "python", "python3", "launchd", "hey-boss"].contains(lower) || lower.hasPrefix("itermserver-") { continue }
        let display: String
        switch lower {
        case "iterm2", "iterm": display = "iTerm"
        case "codex": display = "Codex"
        case "claude": display = "Claude"
        default: display = name
        }
        if names.last != display { names.append(display) }
    }
    return names.joined(separator: " → ")
}

func launchDetails(_ row: Record) -> NSAttributedString {
    let output = NSMutableAttributedString(string: "")
    func entry(_ title: String, _ value: String) {
        output.append(NSAttributedString(string: title + "\n", attributes: [.font: NSFont.systemFont(ofSize: 11, weight: .semibold), .foregroundColor: NSColor.secondaryLabelColor]))
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineSpacing = 3
        output.append(NSAttributedString(string: value + "\n\n", attributes: [.font: NSFont.systemFont(ofSize: 13), .foregroundColor: NSColor.labelColor, .paragraphStyle: paragraph]))
    }
    entry("Source", row.isLocalSource || row.sourceHost == nil ? row.sourceLabel : "Server · \(row.sourceLabel)")
    if let origin = row.origin {
        let chain = launcherChain(origin.launchers)
        if !chain.isEmpty { entry("Launched from", chain) }
        entry("Directory", (origin.cwd as NSString).abbreviatingWithTildeInPath)
        if let git = origin.git {
            if URL(fileURLWithPath: git.root).standardizedFileURL != URL(fileURLWithPath: origin.cwd).standardizedFileURL {
                entry("Repository", (git.root as NSString).abbreviatingWithTildeInPath)
            }
            if let branch = git.branch { entry("Git branch", branch) }
            if let revision = git.revision { entry("Detached HEAD", revision) }
        }
    } else { entry("Launch metadata", "Not recorded for this earlier item.") }
    output.append(NSAttributedString(string: "Task ID\n" + row.taskID, attributes: [.font: NSFont.systemFont(ofSize: 11), .foregroundColor: NSColor.secondaryLabelColor]))
    return output
}

func infoView(_ row: Record) -> NSViewController {
    let controller = NSViewController()
    let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 440, height: 500))
    scroll.hasVerticalScroller = true
    scroll.autohidesScrollers = true
    scroll.drawsBackground = true
    scroll.backgroundColor = .textBackgroundColor
    let text = NSTextView(frame: scroll.contentView.bounds)
    text.drawsBackground = true
    text.backgroundColor = .textBackgroundColor
    text.isEditable = false
    text.isSelectable = true
    text.isVerticallyResizable = true
    text.autoresizingMask = [.width]
    text.textContainerInset = NSSize(width: 20, height: 20)
    text.textContainer!.widthTracksTextView = true
    text.textContainer!.containerSize = NSSize(width: text.frame.width - 40, height: .greatestFiniteMagnitude)
    text.textStorage!.setAttributedString(launchDetails(row))
    scroll.documentView = text
    text.layoutManager!.ensureLayout(for: text.textContainer!)
    scroll.setFrameSize(NSSize(width: 440, height: min(500, text.layoutManager!.usedRect(for: text.textContainer!).height + 40)))
    controller.view = scroll
    return controller
}

final class InfoButton: ActionButton {
    let row: Record
    var popover: NSPopover?
    init(_ row: Record, frame: NSRect) {
        self.row = row
        super.init("", frame: frame, style: .quiet, action: {})
        image = NSImage(systemSymbolName: "ellipsis", accessibilityDescription: "Launch details")
        toolTip = "Launch details"
        setAccessibilityLabel("Launch details")
        invoke = { [weak self] in self?.showInfo() }
    }
    required init?(coder: NSCoder) { return nil }
    func showInfo() {
        let popover = NSPopover()
        popover.behavior = .transient
        popover.contentViewController = infoView(row)
        self.popover = popover
        popover.show(relativeTo: bounds, of: self, preferredEdge: .minY)
    }
}

// Draw above the glass but below content so severity survives vibrancy.
final class SeverityWash: NSView {
    var severity: Severity = .neutral { didSet { needsDisplay = true } }
    var rounded = true { didSet { needsDisplay = true } }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override var allowsVibrancy: Bool { false }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) {
        guard severity != .neutral else { return }
        let dark = effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        let radius: CGFloat = rounded ? 14 : 5
        let shape = NSBezierPath(roundedRect: bounds, xRadius: radius, yRadius: radius)
        severity.color.withAlphaComponent(dark ? 0.20 : 0.12).setFill()
        shape.fill()
        NSGraphicsContext.saveGraphicsState()
        shape.addClip()
        severity.color.withAlphaComponent(0.85).setFill()
        NSRect(x: 0, y: 0, width: 3, height: bounds.height).fill()
        NSGraphicsContext.restoreGraphicsState()
        severity.color.withAlphaComponent(dark ? 0.45 : 0.30).setStroke()
        let border = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        border.lineWidth = 1
        border.stroke()
    }
}

final class Surface: NSView {
    let content = NSView()
    let glass = NSGlassEffectView()
    let severityWash = SeverityWash()
    var severity: Severity {
        get { severityWash.severity }
        set { severityWash.severity = newValue }
    }
    var drawsSurface = true {
        didSet {
            if drawsSurface == oldValue { return }
            layer!.backgroundColor = (drawsSurface ? NSColor.black.withAlphaComponent(0.01) : .clear).cgColor
            if drawsSurface {
                content.removeFromSuperview()
                glass.contentView = content
                addSubview(glass)
            } else {
                glass.contentView = nil
                glass.removeFromSuperview()
                addSubview(content)
            }
            content.frame = bounds
            severityWash.rounded = drawsSurface
        }
    }
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer!.cornerRadius = 14
        layer!.cornerCurve = .continuous
        layer!.masksToBounds = true
        layer!.backgroundColor = NSColor.black.withAlphaComponent(0.01).cgColor
        glass.frame = bounds
        glass.autoresizingMask = [.width, .height]
        glass.cornerRadius = 14
        glass.style = .regular
        glass.wantsLayer = true
        glass.layer!.cornerRadius = 14
        glass.layer!.cornerCurve = .continuous
        glass.layer!.masksToBounds = true
        content.frame = bounds
        content.autoresizingMask = [.width, .height]
        severityWash.frame = content.bounds
        severityWash.autoresizingMask = [.width, .height]
        content.addSubview(severityWash)
        glass.contentView = content
        addSubview(glass)
    }
    required init?(coder: NSCoder) { return nil }
    override func resizeSubviews(withOldSize oldSize: NSSize) {
        super.resizeSubviews(withOldSize: oldSize)
        glass.frame = bounds
        content.frame = bounds
    }
}

func effect(_ frame: NSRect) -> Surface { Surface(frame: frame) }

func panel(_ title: String) -> Panel {
    let panel = Panel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.title = title
    panel.delegate = panel
    panel.level = .statusBar
    panel.isFloatingPanel = true
    panel.becomesKeyOnlyIfNeeded = true
    panel.hidesOnDeactivate = false
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.hasShadow = true
    panel.isReleasedWhenClosed = false
    panel.animationBehavior = .none
    panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
    return panel
}

final class DocumentText: NSTextView {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

final class ReviewSidebar: NSView {
    let heading = NSTextField(labelWithString: "Comments")
    let list = NSScrollView()
    let entries = NSView()
    let composer = NSTextView()
    let composerScroll = NSScrollView()
    let feedback = NSTextField(labelWithString: "Select text to attach a comment, or comment on the document.")
    let add: ActionButton
    let finish: ActionButton
    var comments: [DocumentComment] = []
    var completed = false
    var openComment: (String) -> Void = { _ in }
    var summaryButtons: [ActionButton] = []
    init(addComment: @escaping () -> Void, finishReview: @escaping () -> Void) {
        add = ActionButton("Add comment", frame: .zero, style: .secondary, action: addComment)
        finish = ActionButton("Finish review", frame: .zero, style: .primary, action: finishReview)
        super.init(frame: .zero)
        heading.font = .systemFont(ofSize: 14, weight: .semibold)
        feedback.font = .systemFont(ofSize: 11)
        feedback.textColor = .secondaryLabelColor
        feedback.lineBreakMode = .byTruncatingTail
        feedback.toolTip = feedback.stringValue
        composer.font = .systemFont(ofSize: 13)
        composer.isRichText = false
        composer.isAutomaticQuoteSubstitutionEnabled = false
        composer.textContainerInset = NSSize(width: 8, height: 8)
        composer.setAccessibilityLabel("Write a document comment")
        composerScroll.documentView = composer
        composerScroll.hasVerticalScroller = true
        composerScroll.borderType = .bezelBorder
        composerScroll.autohidesScrollers = true
        list.documentView = entries
        list.drawsBackground = false
        list.hasVerticalScroller = true
        list.autohidesScrollers = true
        for view in [heading, list] { addSubview(view) }
        add.isHidden = true
        finish.isHidden = true
        add.controlSize = .regular
        finish.controlSize = .regular
    }
    required init?(coder: NSCoder) { nil }
    override func resizeSubviews(withOldSize oldSize: NSSize) {
        super.resizeSubviews(withOldSize: oldSize)
        arrange()
    }
    func arrange() {
        let width = bounds.width
        heading.frame = NSRect(x: 18, y: bounds.height - 42, width: width - 36, height: 22)
        list.frame = NSRect(x: 0, y: 12, width: width, height: max(0, bounds.height - 66))
        heading.stringValue = comments.isEmpty ? "Comments" : "Comments · \(comments.count)"
        entries.subviews.forEach { $0.removeFromSuperview() }
        summaryButtons = []
        let textWidth = width - 36
        var rows: [(NSView, CGFloat)] = []
        for comment in comments {
            let container = NSView()
            let displayText = String(comment.text.prefix(240))
            let text = NSTextField(wrappingLabelWithString: displayText)
            text.font = .systemFont(ofSize: 13)
            text.isSelectable = true
            text.maximumNumberOfLines = 3
            text.lineBreakMode = .byTruncatingTail
            text.toolTip = comment.text
            let measured = ceil(text.sizeThatFits(NSSize(width: textWidth, height: .greatestFiniteMagnitude)).height)
            let textHeight = min(54, measured)
            var height = textHeight + 34
            if let quote = comment.quote {
                let quoteLabel = NSTextField(wrappingLabelWithString: comment.selection.map { "Lines \($0.line_start)–\($0.line_end) · " } .map { $0 + quote } ?? quote)
                quoteLabel.font = .systemFont(ofSize: 11)
                quoteLabel.textColor = .secondaryLabelColor
                quoteLabel.maximumNumberOfLines = 3
                quoteLabel.lineBreakMode = .byTruncatingTail
                quoteLabel.toolTip = quote
                let quoteHeight = min(45, ceil(quoteLabel.sizeThatFits(NSSize(width: textWidth - 10, height: .greatestFiniteMagnitude)).height))
                quoteLabel.frame = NSRect(x: 10, y: textHeight + 26, width: textWidth - 10, height: quoteHeight)
                let accent = NSBox(frame: NSRect(x: 0, y: textHeight + 26, width: 2, height: quoteHeight))
                accent.boxType = .custom; accent.fillColor = .separatorColor; accent.borderWidth = 0
                container.addSubview(accent); container.addSubview(quoteLabel)
                height += quoteHeight + 8
            }
            text.frame = NSRect(x: 0, y: 20, width: textWidth, height: textHeight)
            container.addSubview(text)
            let divider = NSBox(frame: NSRect(x: 0, y: 2, width: textWidth, height: 1))
            divider.boxType = .separator; container.addSubview(divider)
            let open = ActionButton("Open comment", frame: NSRect(x: 0, y: 8, width: 120, height: 22), style: .link) { [weak self] in self?.openComment(comment.id) }
            open.font = .systemFont(ofSize: 11)
            for child in container.subviews {
                if let box = child as? NSBox, box.boxType == .separator { continue }
                child.frame.origin.y += 22
            }
            height += 22; container.addSubview(open); summaryButtons.append(open)
            rows.append((container, height))
        }
        if comments.isEmpty {
            let empty = NSTextField(wrappingLabelWithString: "No comments yet.\nSelect text to start a comment. Saved notes will appear here.")
            empty.font = .systemFont(ofSize: 12)
            empty.textColor = .secondaryLabelColor
            empty.frame = NSRect(x: 0, y: 0, width: textWidth, height: 58)
            rows.append((empty, 66))
        }
        let total = rows.reduce(CGFloat(8)) { $0 + $1.1 }
        entries.frame = NSRect(x: 0, y: 0, width: width, height: max(list.contentSize.height, total))
        var top = entries.frame.height - 8
        for (view, height) in rows { top -= height; view.frame = NSRect(x: 18, y: top, width: textWidth, height: height); entries.addSubview(view) }
        list.contentView.scroll(to: NSPoint(x: 0, y: max(0, entries.frame.height - list.contentSize.height)))
        add.isEnabled = !completed
        finish.isEnabled = !completed
        composer.isEditable = !completed
        if completed { feedback.stringValue = "Comments saved." }
    }
}

final class OpaqueCommentSurface: NSView {
    override var isOpaque: Bool { true }
    override var allowsVibrancy: Bool { false }
    override func draw(_ dirtyRect: NSRect) { NSColor.windowBackgroundColor.setFill(); dirtyRect.fill() }
}
final class SelectionCommentButton: ActionButton {
    override var allowsVibrancy: Bool { false }
    override func draw(_ dirtyRect: NSRect) {
        NSColor.controlBackgroundColor.setFill()
        NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 8, yRadius: 8).fill()
        NSColor.separatorColor.setStroke()
        NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 8, yRadius: 8).stroke()
        super.draw(dirtyRect)
    }
}

final class FloatingCommentEditor: NSPanel, NSWindowDelegate {
    let quote = NSTextField(wrappingLabelWithString: "")
    let scroll: NSScrollView
    let status: NSTextField
    let sendButton: ActionButton
    var requestClose: () -> Void = {}
    var sendComment: () -> Void = {}
    init(scroll: NSScrollView, status: NSTextField) {
        self.scroll = scroll; self.status = status
        sendButton = ActionButton("Send", frame: NSRect(x: 242, y: 10, width: 76, height: 28), style: .primary, action: {})
        super.init(contentRect: NSRect(x: 0, y: 0, width: 336, height: 270), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        title = "Comment"; level = .floating; hidesOnDeactivate = false
        isOpaque = true; backgroundColor = .windowBackgroundColor
        contentView = OpaqueCommentSurface(frame: NSRect(x: 0, y: 0, width: 336, height: 270))
        hasShadow = false
        isReleasedWhenClosed = false; delegate = self
        quote.font = .systemFont(ofSize: 12); quote.textColor = .secondaryLabelColor
        quote.maximumNumberOfLines = 3; quote.lineBreakMode = .byTruncatingTail
        quote.frame = NSRect(x: 18, y: 204, width: 300, height: 48)
        scroll.frame = NSRect(x: 18, y: 42, width: 300, height: 150)
        status.frame = NSRect(x: 18, y: 14, width: 212, height: 18)
        sendButton.controlSize = .regular
        sendButton.frame = NSRect(x: 242, y: 10, width: 76, height: 28)
        sendButton.toolTip = "Send comment (⌘Enter)"
        sendButton.invoke = { [weak self] in self?.sendComment() }
        for view in [quote, scroll, status, sendButton] { contentView?.addSubview(view) }
        if let composer = scroll.documentView as? NSTextView {
            composer.frame = NSRect(x: 0, y: 0, width: scroll.contentSize.width, height: 150)
            composer.isVerticallyResizable = true; composer.isHorizontallyResizable = false
            composer.autoresizingMask = [.width]
            composer.textContainer?.widthTracksTextView = true
        }
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool { requestClose(); return false }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
        if modifiers == .command && [36, 76].contains(event.keyCode) { sendComment(); return true }
        if event.keyCode == 53 { requestClose(); return true }
        return super.performKeyEquivalent(with: event)
    }
    override func sendEvent(_ event: NSEvent) {
        if event.type == .keyDown && performKeyEquivalent(with: event) { return }
        super.sendEvent(event)
    }
}

final class Preview: NSWindow, NSWindowDelegate, NSTextViewDelegate, WKNavigationDelegate {
    let reviewID: String
    var reviewRecord: Record
    var reviewSidebar: ReviewSidebar?
    var commentEditor: FloatingCommentEditor?
    var editorCloseAfterSave = false
    var commentPosition: NSPoint?
    var saveComment: (String, String, String?, String?, DocumentSelection?, @escaping (Result<Record, Error>) -> Void) -> Void = { _, _, _, _, _, completion in completion(.failure(StorageError(description: "Review storage is unavailable"))) }
    var finishReview: (String, @escaping (Result<Record, Error>) -> Void) -> Void = { _, completion in completion(.failure(StorageError(description: "Review storage is unavailable"))) }
    var reviewConstraints: [NSLayoutConstraint] = []
    var sidebarCollapsed = true
    var selectedQuote: String?
    var selectedSource: DocumentSelection?
    var queuedSelection: Any?
    var queuedCommentID: String?
    var editingCommentID: String?
    var autosaveTimer: Timer?
    var saveInFlight = false
    var finishAfterSave = false
    var closeAfterSave = false
    var reviewClosing = false
    var selectionAction: ActionButton?
    var commentsToggle: ActionButton?
    var selectionObserver: Any?
    var fallbackScroll: NSScrollView?
    var browser: WKWebView?
    var renderedDocument: String?
    var rendererReloaded = false
    var readerLoaded = false
    var onClose: () -> Void = {}
    let text = DocumentText()
    let openURL: (URL) -> Void
    var linkButtons: [ActionButton] = []
    init(_ row: Record, openURL: @escaping (URL) -> Void) {
        self.openURL = openURL
        self.reviewID = row.taskID
        self.reviewRecord = row
        super.init(contentRect: NSRect(x: 0, y: 0, width: 720, height: 640), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        title = (row.title ?? "Untitled")
        subtitle = (row.project ?? "Notifications") + (row.documentName.map { " · " + $0 } ?? "")
        level = .floating
        hidesOnDeactivate = false
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        isReleasedWhenClosed = false
        minSize = NSSize(width: 420, height: 320)
        delegate = self
        let scroll = NSScrollView(frame: contentView!.bounds)
        scroll.autoresizingMask = [.width, .height]
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        text.frame = scroll.contentView.bounds
        text.isEditable = false
        text.isSelectable = true
        text.delegate = self
        text.isVerticallyResizable = true
        text.isHorizontallyResizable = false
        text.autoresizingMask = [.width]
        text.textContainerInset = NSSize(width: 40, height: 32)
        text.textContainer!.widthTracksTextView = true
        text.textContainer!.containerSize = NSSize(width: text.frame.width - 80, height: .greatestFiniteMagnitude)
        text.textStorage!.setAttributedString(markdown(row.question, size: 15, color: .textColor))
        text.linkTextAttributes = [.foregroundColor: NSColor.linkColor, .underlineStyle: NSUnderlineStyle.single.rawValue]
        scroll.documentView = text
        fallbackScroll = scroll
        contentView!.addSubview(scroll)
        var links: [(URL, String)] = []
        text.textStorage!.enumerateAttribute(.link, in: NSRange(location: 0, length: text.textStorage!.length)) { value, range, _ in
            if let url = value as? URL, !links.contains(where: { $0.0 == url }) {
                links.append((url, (self.text.string as NSString).substring(with: range)))
            }
        }
        if !links.isEmpty {
            let height = min(CGFloat(links.count) * 48 + 16, 160)
            let tray = NSScrollView(frame: NSRect(x: 0, y: 0, width: 720, height: height))
            tray.autoresizingMask = [.width]
            tray.hasVerticalScroller = true
            tray.autohidesScrollers = true
            tray.drawsBackground = true
            tray.backgroundColor = .windowBackgroundColor
            let body = NSView(frame: NSRect(x: 0, y: 0, width: tray.contentSize.width, height: CGFloat(links.count) * 48 + 16))
            body.autoresizingMask = [.width]
            for (index, link) in links.enumerated() {
                let button = ActionButton(link.1, frame: NSRect(x: 32, y: body.frame.height - 8 - CGFloat(index + 1) * 48, width: body.frame.width - 64, height: 44), style: .link) { openURL(link.0) }
                button.autoresizingMask = [.width]
                button.alignment = .left
                button.font = .systemFont(ofSize: 14, weight: .medium)
                button.setAccessibilityLabel("Open \(link.1)")
                button.image = NSImage(systemSymbolName: "arrow.up.right", accessibilityDescription: nil)
                button.imagePosition = .imageTrailing
                button.toolTip = link.0.absoluteString
                button.cell!.lineBreakMode = .byTruncatingTail
                body.addSubview(button)
                linkButtons.append(button)
            }
            tray.documentView = body
            tray.contentView.scroll(to: NSPoint(x: 0, y: max(0, body.frame.height - height)))
            contentView!.addSubview(tray)
            let divider = NSBox(frame: NSRect(x: 0, y: height - 2, width: 720, height: 5))
            divider.boxType = .separator
            divider.autoresizingMask = [.width]
            contentView!.addSubview(divider)
            scroll.frame = NSRect(x: 0, y: height, width: 720, height: 640 - height)
        }
        initialFirstResponder = text
        let accessory = NSTitlebarAccessoryViewController()
        accessory.layoutAttribute = .right
        let contextWidth: CGFloat = row.visualSeverity == .neutral ? 66 : 154
        let context = NSView(frame: NSRect(x: 0, y: 0, width: contextWidth, height: 28))
        context.addSubview(IconBadge(row, frame: NSRect(x: 0, y: 2, width: 24, height: 24)))
        if row.visualSeverity != .neutral { context.addSubview(severityLabel(row, frame: NSRect(x: 32, y: 6, width: 84, height: 16))) }
        context.addSubview(InfoButton(row, frame: NSRect(x: contextWidth - 34, y: 0, width: 32, height: 28)))
        accessory.view = context
        addTitlebarAccessoryViewController(accessory)
        if row.commentsEnabled == true { setContentSize(NSSize(width: 960, height: 680)) }
        center()
        if row.commentsEnabled == true { installReviewSidebar() }
        if let attachment = row.attachment {
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                do {
                    _ = try attachment.validatedImage()
                    let html = "<!doctype html><html><head><meta name=\"viewport\" content=\"width=device-width\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'\"><style>:root{color-scheme:light dark}body{margin:0;background:Canvas;color:CanvasText;font:14px -apple-system}figure{margin:24px;display:flex;justify-content:center}img{max-width:100%;height:auto;object-fit:contain}</style></head><body><figure><img alt=\"Review image\" src=\"data:" + attachment.mime + ";base64," + attachment.data + "\"></figure></body></html>"
                    onMain { [weak self] in self?.showRenderedMarkdown(html) }
                } catch { reportFailure(error) }
            }
        } else { prepareMarkdownReader(row.question) }
    }
    func prepareMarkdownReader(_ source: String) {
        guard source.utf8.count <= 2 * 1024 * 1024 else { return }
        let executable = ProcessInfo.processInfo.environment["HEY_BOSS_CLI_PATH"] ?? "/opt/homebrew/bin/hey-boss"
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent("hey-boss-reader-" + UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: directory) }
            do {
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
                let input = directory.appendingPathComponent("document.md")
                let output = directory.appendingPathComponent("document.html")
                try Data(source.utf8).write(to: input, options: .atomic)
                let process = Process()
                process.executableURL = URL(fileURLWithPath: executable)
                process.arguments = ["render-markdown", input.path, output.path, "--source-map"]
                process.standardOutput = FileHandle.nullDevice
                process.standardError = FileHandle.nullDevice
                try process.run()
                let deadline = DispatchTime.now().uptimeNanoseconds + 20_000_000_000
                while process.isRunning && DispatchTime.now().uptimeNanoseconds < deadline { Thread.sleep(forTimeInterval: 0.05) }
                if process.isRunning {
                    process.terminate()
                    let cleanup = DispatchTime.now().uptimeNanoseconds + 2_000_000_000
                    while process.isRunning && DispatchTime.now().uptimeNanoseconds < cleanup { Thread.sleep(forTimeInterval: 0.05) }
                    if process.isRunning { kill(process.processIdentifier, SIGKILL) }
                    throw StorageError(description: "Markdown renderer timed out")
                }
                guard process.terminationStatus == 0 else { throw StorageError(description: "Markdown renderer failed") }
                let attributes = try FileManager.default.attributesOfItem(atPath: output.path)
                guard (attributes[.size] as? NSNumber)?.intValue ?? Int.max <= 8 * 1024 * 1024 else { throw StorageError(description: "Rendered Markdown is too large") }
                let html = try String(contentsOf: output, encoding: .utf8)
                onMain { [weak self] in self?.showRenderedMarkdown(html) }
            } catch { reportFailure(error) }
        }
    }
    func showRenderedMarkdown(_ html: String) {
        renderedDocument = html
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = false
        let web = WKWebView(frame: contentView!.bounds, configuration: configuration)
        web.autoresizingMask = [.width, .height]
        web.navigationDelegate = self
        web.allowsMagnification = true
        web.setAccessibilityLabel("Markdown update document")
        for view in contentView!.subviews { view.isHidden = true }
        contentView!.addSubview(web)
        browser = web
        web.loadHTMLString(html, baseURL: nil)
        if reviewRecord.commentsEnabled == true { installReviewSidebar() }
    }
    func installReviewSidebar() {
        guard browser != nil || fallbackScroll != nil, let contentView else { return }
        minSize = NSSize(width: 720, height: 480)
        let sidebar = reviewSidebar ?? ReviewSidebar(addComment: { [weak self] in self?.submitComment() }, finishReview: { [weak self] in self?.submitReview() })
        reviewSidebar = sidebar
        if sidebar.superview == nil { contentView.addSubview(sidebar) }
        sidebar.composer.delegate = self
        sidebar.openComment = { [weak self] id in self?.openSavedComment(id) }
        sidebar.comments = reviewRecord.comments ?? []
        sidebar.completed = reviewRecord.status != "pending"
        if commentsToggle == nil {
            let accessory = NSTitlebarAccessoryViewController()
            accessory.layoutAttribute = .right
            let controls = NSView(frame: NSRect(x: 0, y: 0, width: 116, height: 30))
            let toggle = ActionButton("Comments", frame: NSRect(x: 0, y: 1, width: 106, height: 28), style: .secondary) { [weak self] in
                guard let self else { return }
                self.sidebarCollapsed.toggle(); self.layoutReview()
            }
            toggle.image = NSImage(systemSymbolName: "sidebar.right", accessibilityDescription: nil)
            toggle.imagePosition = .imageLeading
            commentsToggle = toggle
            controls.addSubview(toggle)
            accessory.view = controls; addTitlebarAccessoryViewController(accessory)
            let action = SelectionCommentButton("Add comment", frame: NSRect(x: 0, y: 0, width: 132, height: 32), style: .link) { [weak self] in self?.beginSelectedComment() }
            action.isBordered = false; action.controlSize = .regular
            action.image = NSImage(systemSymbolName: "text.bubble", accessibilityDescription: nil)
            action.imagePosition = .imageLeading
            action.isHidden = true; contentView.addSubview(action); selectionAction = action
            selectionObserver = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseUp]) { [weak self] event in
                guard let self, event.window === self, let browser = self.browser else { return event }
                let point = browser.convert(event.locationInWindow, from: nil)
                if browser.bounds.contains(point) {
                    DispatchQueue.main.async { [weak self] in self?.inspectSelection(at: point) }
                }
                return event
            }
        }
        layoutReview()
    }
    func layoutReview() {
        guard let web = (browser as NSView?) ?? fallbackScroll, let sidebar = reviewSidebar, let contentView else { return }
        NSLayoutConstraint.deactivate(reviewConstraints)
        web.translatesAutoresizingMaskIntoConstraints = false
        sidebar.translatesAutoresizingMaskIntoConstraints = false
        sidebar.isHidden = sidebarCollapsed
        reviewConstraints = [web.leadingAnchor.constraint(equalTo: contentView.leadingAnchor), web.topAnchor.constraint(equalTo: contentView.topAnchor), web.bottomAnchor.constraint(equalTo: contentView.bottomAnchor)]
        if sidebarCollapsed { reviewConstraints.append(web.trailingAnchor.constraint(equalTo: contentView.trailingAnchor)) }
        else {
            reviewConstraints += [web.trailingAnchor.constraint(equalTo: sidebar.leadingAnchor, constant: -1), sidebar.trailingAnchor.constraint(equalTo: contentView.trailingAnchor), sidebar.topAnchor.constraint(equalTo: contentView.topAnchor), sidebar.bottomAnchor.constraint(equalTo: contentView.bottomAnchor), sidebar.widthAnchor.constraint(equalToConstant: 280)]
        }
        NSLayoutConstraint.activate(reviewConstraints)
        commentsToggle?.title = sidebarCollapsed ? "Comments" : "Hide comments"
        sidebar.arrange()
        contentView.layoutSubtreeIfNeeded()
    }
    func inspectSelection(at point: NSPoint? = nil, beginEditor: Bool = false) {
        guard let browser, !saveInFlight else { return }
        let x = point?.x ?? -1
        let y = point.map { browser.isFlipped ? $0.y : browser.bounds.height - $0.y } ?? -1
        let script = """
        (()=>{
          const hit=document.elementFromPoint(\(Double(x)),\(Double(y))),m=hit?.closest('mark[data-review-id]');
          if(m){const r=m.getBoundingClientRect();return {commentID:m.dataset.reviewId,x:r.right,y:r.top};}
          const s=window.getSelection();if(!s.rangeCount||s.isCollapsed)return null;
          const range=s.getRangeAt(0),rect=range.getBoundingClientRect();let first=Infinity,last=0;
          for(const span of document.querySelectorAll('[data-source-start]')){
            if(!range.intersectsNode(span))continue;
            const part=document.createRange();part.selectNodeContents(span);
            if(part.compareBoundaryPoints(Range.START_TO_START,range)<0)part.setStart(range.startContainer,range.startOffset);
            if(part.compareBoundaryPoints(Range.END_TO_END,range)>0)part.setEnd(range.endContainer,range.endOffset);
            if(!part.toString().trim())continue;
            first=Math.min(first,Number(span.dataset.sourceStart));last=Math.max(last,Number(span.dataset.sourceEnd));
          }
          return {quote:s.toString().slice(0,2000),x:rect.right,y:rect.bottom,lineStart:Number.isFinite(first)?first:null,lineEnd:last};
        })()
        """

        browser.evaluateJavaScript(script) { [weak self] value, _ in
            self?.handleSelection(value, browser: browser)
            if beginEditor, self?.selectedQuote != nil { self?.beginSelectedComment() }
        }
    }
    func openSavedComment(_ id: String) {
        guard let comment = reviewRecord.comments?.first(where: { $0.id == id }), let sidebar = reviewSidebar else { return }
        let draft = sidebar.composer.string.trimmingCharacters(in: .whitespacesAndNewlines)
        if !draft.isEmpty {
            guard let oldID = editingCommentID, reviewRecord.comments?.first(where: { $0.id == oldID })?.text == draft, !saveInFlight else { queuedCommentID = id; submitComment(); return }
        }
        selectedQuote = comment.quote; selectedSource = comment.selection; editingCommentID = id
        sidebar.composer.string = comment.text
        if let browser, let data = try? JSONSerialization.data(withJSONObject: [id]), let encoded = String(data: data, encoding: .utf8) {
            let script = "(()=>{const id=\(encoded)[0],m=[...document.querySelectorAll('mark[data-review-id]')].find(m=>m.dataset.reviewId===id);if(!m)return null;m.scrollIntoView({block:'center'});const r=m.getBoundingClientRect();return {x:r.right,y:r.top};})()"
            browser.evaluateJavaScript(script) { [weak self] value, _ in
                guard let self, self.editingCommentID == id else { return }
                if let coordinates = value as? [String: Any] { self.updateCommentPosition(coordinates, browser: browser) }
                self.beginSelectedComment()
            }
        } else { beginSelectedComment() }
    }
    func updateCommentPosition(_ coordinates: [String: Any], browser: WKWebView) {
        guard let content = contentView, let x = coordinates["x"] as? Double, let y = coordinates["y"] as? Double else { return }
        let local = browser.convert(NSPoint(x: x, y: browser.isFlipped ? y : browser.bounds.height - y), to: content)
        commentPosition = convertPoint(toScreen: content.convert(local, to: nil))
    }
    func handleSelection(_ value: Any?, browser: WKWebView) {
        if let result = value as? [String: Any] { updateCommentPosition(result, browser: browser) }
        if let result = value as? [String: Any], let id = result["commentID"] as? String { openSavedComment(id); return }
        guard reviewRecord.status == "pending" else { selectionAction?.isHidden = true; return }
        guard let selection = value as? [String: Any], let quote = selection["quote"] as? String, !quote.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { selectionAction?.isHidden = true; return }
        if quote != selectedQuote, let sidebar = reviewSidebar, !sidebar.composer.string.isEmpty {
            let draft = sidebar.composer.string.trimmingCharacters(in: .whitespacesAndNewlines)
            guard let id = editingCommentID, reviewRecord.comments?.first(where: { $0.id == id })?.text == draft, !saveInFlight else { queuedSelection = value; submitComment(); return }
            sidebar.composer.string = ""; editingCommentID = nil
            commentEditor?.orderOut(nil)
        }
        selectedQuote = quote
        selectedSource = nil
        if let first = selection["lineStart"] as? Int, let last = selection["lineEnd"] as? Int { selectedSource = sourceSelection(first: first, last: last) }
        guard let button = selectionAction, let content = contentView else { return }
        let jsY = selection["y"] as? Double ?? 0
        let point = browser.convert(NSPoint(x: selection["x"] as? Double ?? 0, y: browser.isFlipped ? jsY : browser.bounds.height - jsY), to: content)
        button.frame.origin = NSPoint(x: min(max(8, point.x - 40), max(8, browser.frame.maxX - 140)), y: min(max(8, point.y - 38), content.bounds.height - 40))
        button.isHidden = false; content.addSubview(button, positioned: .above, relativeTo: nil)
    }
    var sourceLineOffset: Int {
        let extensionName = (reviewRecord.documentName as NSString?)?.pathExtension.lowercased()
        return reviewRecord.documentName != nil && !["md", "markdown", "mdown"].contains(extensionName ?? "") && reviewRecord.attachment == nil ? 1 : 0
    }
    func sourceSelection(first: Int, last: Int) -> DocumentSelection? {
        let lines = reviewRecord.question.components(separatedBy: "\n"), offset = sourceLineOffset
        guard first > offset, last >= first, last <= lines.count else { return nil }
        // The source-file display fence adds a newline even when the file has none.
        let hasNewline = last < lines.count && !(offset == 1 && last == lines.count - 1)
        let source = lines[(first - 1)...(last - 1)].joined(separator: "\n") + (hasNewline ? "\n" : "")
        return DocumentSelection(line_start: first - offset, line_end: last - offset, source_text: source)
    }
    func beginSelectedComment() {
        guard let sidebar = reviewSidebar else { return }
        selectionAction?.isHidden = true
        if sidebar.composer.string.isEmpty { editingCommentID = nil }
        let editor = commentEditor ?? FloatingCommentEditor(scroll: sidebar.composerScroll, status: sidebar.feedback)
        if commentEditor == nil {
            commentEditor = editor; addChildWindow(editor, ordered: .above)
            editor.requestClose = { [weak self] in self?.closeCommentEditor() }
            editor.sendComment = { [weak self] in self?.closeCommentEditor() }
        }
        editor.quote.stringValue = (selectedSource.map { "Lines \($0.line_start)–\($0.line_end)\n" } ?? "") + (selectedQuote ?? "Document comment")
        editor.quote.toolTip = selectedQuote
        sidebar.feedback.stringValue = sidebar.composer.string.isEmpty ? "Auto-saves · ⌘Enter to send" : "Saved · ⌘Enter to send"
        let area = screen?.visibleFrame ?? frame
        let target = commentPosition ?? NSPoint(x: frame.maxX - 360, y: frame.maxY - 100)
        var sideX = target.x + 12
        if let browser {
            let documentFrame = convertToScreen(browser.convert(browser.bounds, to: nil))
            if documentFrame.maxX + editor.frame.width + 24 <= area.maxX { sideX = documentFrame.maxX + 12 }
            else if documentFrame.minX - editor.frame.width - 24 >= area.minX { sideX = documentFrame.minX - editor.frame.width - 12 }
        }
        let origin = NSPoint(x: min(max(area.minX + 12, sideX), area.maxX - editor.frame.width - 12), y: min(max(area.minY + 12, target.y - editor.frame.height), area.maxY - editor.frame.height - 12))
        updateEditorState()
        browser?.evaluateJavaScript("window.getSelection().removeAllRanges()") { _, _ in }
        editor.setFrameOrigin(origin); editor.makeKeyAndOrderFront(nil)
        editor.makeFirstResponder(sidebar.composer)
    }
    func updateEditorState() {
        guard let editor = commentEditor, let sidebar = reviewSidebar else { return }
        editor.sendButton.isEnabled = !sidebar.composer.string.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !saveInFlight && !sidebar.completed
        editor.sendButton.title = saveInFlight && editorCloseAfterSave ? "Sending…" : "Send"
    }
    func closeCommentEditor() {
        guard let sidebar = reviewSidebar else { return }
        let draft = sidebar.composer.string.trimmingCharacters(in: .whitespacesAndNewlines)
        if !draft.isEmpty {
            if let id = editingCommentID, reviewRecord.comments?.first(where: { $0.id == id })?.text == draft, !saveInFlight { hideCommentEditor() }
            else { editorCloseAfterSave = true; submitComment() }
        } else if saveInFlight { editorCloseAfterSave = true }
        else { hideCommentEditor() }
        updateEditorState()
    }
    func hideCommentEditor() {
        commentEditor?.orderOut(nil)
        reviewSidebar?.composer.string = ""
        editingCommentID = nil; selectedQuote = nil; selectedSource = nil
        updateEditorState()
        makeFirstResponder(browser ?? text)
    }
    func textDidChange(_ notification: Notification) {
        guard notification.object as? NSTextView === reviewSidebar?.composer else { return }
        autosaveTimer?.invalidate()
        reviewSidebar?.feedback.stringValue = "Saving…"
        updateEditorState()
        autosaveTimer = Timer.scheduledTimer(withTimeInterval: 0.8, repeats: false) { [weak self] _ in self?.submitComment() }
    }
    func submitComment() {
        autosaveTimer?.invalidate(); autosaveTimer = nil
        guard let sidebar = reviewSidebar, !saveInFlight, !sidebar.completed else { return }
        let draft = sidebar.composer.string.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !draft.isEmpty else { sidebar.feedback.stringValue = "Write a comment to save it."; return }
        saveInFlight = true
        updateEditorState()
        let quote = selectedQuote
        let oldID = editingCommentID
        saveComment(reviewID, draft, quote, oldID, selectedSource) { [weak self] result in
            guard let self, let sidebar = self.reviewSidebar else { return }
            self.saveInFlight = false
            self.updateEditorState()
            switch result {
            case .success(let row):
                self.reviewRecord = row; sidebar.comments = row.comments ?? []
                self.editingCommentID = oldID ?? row.comments?.last?.id
                sidebar.feedback.stringValue = "Saved"
                self.highlightComments()
            case .failure(let error):
                self.finishAfterSave = false; self.closeAfterSave = false; self.reviewClosing = false; self.editorCloseAfterSave = false
                sidebar.feedback.stringValue = "Couldn’t save. Keep this window open to retry."
                self.commentEditor?.makeKeyAndOrderFront(nil)
                sidebar.feedback.toolTip = String(describing: error)
                sidebar.arrange(); self.updateEditorState(); return
            }
            sidebar.arrange(); self.updateEditorState()
            if sidebar.composer.string.trimmingCharacters(in: .whitespacesAndNewlines) != draft { self.submitComment(); return }
            if let id = self.queuedCommentID {
                self.queuedCommentID = nil; self.openSavedComment(id)
            } else if let selection = self.queuedSelection, let browser = self.browser {
                self.queuedSelection = nil; sidebar.composer.string = ""; self.editingCommentID = nil; self.commentEditor?.orderOut(nil)
                self.handleSelection(selection, browser: browser)
            }
            if self.editorCloseAfterSave { self.editorCloseAfterSave = false; self.hideCommentEditor() }
            if self.finishAfterSave { self.finishAfterSave = false; self.completeReview() }
        }
    }
    func submitReview() {
        guard let sidebar = reviewSidebar, !sidebar.completed else { return }
        let draft = sidebar.composer.string.trimmingCharacters(in: .whitespacesAndNewlines)
        if !draft.isEmpty {
            if let id = editingCommentID, reviewRecord.comments?.first(where: { $0.id == id })?.text == draft, !saveInFlight { completeReview() }
            else { finishAfterSave = true; submitComment() }
        } else if saveInFlight { finishAfterSave = true }
        else { completeReview() }
    }
    func completeReview() {
        finishReview(reviewID) { [weak self] result in
            guard let self, let sidebar = self.reviewSidebar else { return }
            switch result {
            case .success(let row):
                self.reviewRecord = row; sidebar.completed = true
                if self.closeAfterSave { self.closeAfterSave = false; self.close() }
            case .failure(let error): self.closeAfterSave = false; self.reviewClosing = false; sidebar.feedback.stringValue = String(describing: error)
            }
            sidebar.arrange()
        }
    }
    func highlightComments() {
        guard let browser, let data = try? JSONSerialization.data(withJSONObject: (reviewRecord.comments ?? []).compactMap { comment -> [String: String]? in guard let quote = comment.quote else { return nil }; var result = ["id": comment.id, "quote": quote, "text": comment.text]
            if let selection = comment.selection { result["lineStart"] = String(selection.line_start + self.sourceLineOffset); result["lineEnd"] = String(selection.line_end + self.sourceLineOffset) }
            return result }), let json = String(data: data, encoding: .utf8) else { return }
        // Only app-generated native evaluation runs; document scripts stay disabled.
        let script = """
        (()=>{
          for(const m of document.querySelectorAll('mark[data-review-id]'))m.replaceWith(...m.childNodes);
          document.body.normalize();
          const comments=\(json), walker=document.createTreeWalker(document.body,NodeFilter.SHOW_TEXT);
          const runs=[];let flat='',node,space=false;
          while(node=walker.nextNode()){
            if(node.parentElement.closest('script,style'))continue;
            const offsets=[],start=flat.length,t=node.textContent;
            for(let i=0;i<t.length;i++){
              if(/\\s/.test(t[i])){if(space)continue;flat+=' ';space=true;}
              else{flat+=t[i];space=false;}
              offsets.push(i);
            }
            runs.push({node,start,end:flat.length,offsets,line:Number(node.parentElement.closest('[data-source-start]')?.dataset.sourceStart||0)});
          }
          const matches=[];
          for(const c of comments){const q=c.quote.replace(/\\s+/g,' ').trim();if(!q)continue;const located=c.lineStart?runs.filter(r=>r.line>=Number(c.lineStart)&&r.line<=Number(c.lineEnd)):runs;const lower=located[0]?.start??0,upper=located[located.length-1]?.end??flat.length;const start=flat.indexOf(q,lower);if(start>=0&&start+q.length<=upper)matches.push({start,end:start+q.length,c});}
          for(const run of runs){
            const segments=matches.filter(m=>m.start<run.end&&m.end>run.start).map(m=>({start:run.offsets[Math.max(0,m.start-run.start)],end:Math.min(m.end,run.end)===run.end?run.node.length:run.offsets[Math.min(m.end,run.end)-run.start],c:m.c})).sort((a,b)=>b.start-a.start);
            let limit=run.node.length;
            for(const seg of segments){if(seg.end>limit||seg.end<=seg.start)continue;const r=document.createRange();r.setStart(run.node,seg.start);r.setEnd(run.node,seg.end);const mark=document.createElement('mark');mark.dataset.reviewId=seg.c.id;mark.title=seg.c.text;mark.style.cssText='background:rgba(255,193,7,.23);color:inherit;border-bottom:2px solid rgba(215,153,0,.55);border-radius:2px;cursor:pointer';r.surroundContents(mark);limit=seg.start;}
          }
        })()
        """
        browser.evaluateJavaScript(script) { _, _ in }

    }
    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard let url = navigationAction.request.url else { decisionHandler(.cancel); return }
        if navigationAction.navigationType == .linkActivated {
            if url.scheme == "about", url.fragment != nil { decisionHandler(.allow); return }
            decisionHandler(.cancel)
            if ["https", "http", "file", "mailto"].contains(url.scheme?.lowercased() ?? "") { openURL(url) }
        } else {
            decisionHandler(url.scheme == "about" ? .allow : .cancel)
        }
    }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { readerLoaded = true; highlightComments() }
    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        guard !rendererReloaded, let renderedDocument else { return }
        rendererReloaded = true
        webView.loadHTMLString(renderedDocument, baseURL: nil)
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard reviewRecord.commentsEnabled == true, reviewRecord.status == "pending" else { return true }
        if reviewClosing { return false }
        reviewClosing = true; closeAfterSave = true
        sidebarCollapsed = false; layoutReview()
        submitReview()
        return false
    }
    func windowWillClose(_ notification: Notification) {
        autosaveTimer?.invalidate()
        if let commentEditor { removeChildWindow(commentEditor); commentEditor.close() }
        if let selectionObserver { NSEvent.removeMonitor(selectionObserver) }
        onClose()
    }
    func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
        guard let url = link as? URL else { return false }
        openURL(url)
        return true
    }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let modifiers = event.modifierFlags.intersection([.control, .command, .option, .shift])
        let key = event.charactersIgnoringModifiers?.lowercased()
        if modifiers == [.command, .shift] && key == "m", reviewSidebar != nil {
            if browser != nil {
                inspectSelection(beginEditor: true)
            } else {
                let range = text.selectedRange()
                if range.length > 0 { selectedQuote = (text.string as NSString).substring(with: range) }
                beginSelectedComment()
            }
            return true
        }
        if (modifiers == .command || modifiers == .control) && key == "w" {
            performClose(nil)
            return true
        }
        if modifiers == .command && firstResponder === text {
            if key == "a" { text.selectAll(nil); return true }
            if key == "c" { text.copy(nil); return true }
        }
        return super.performKeyEquivalent(with: event)
    }
    override func sendEvent(_ event: NSEvent) {
        if event.type == .keyDown && performKeyEquivalent(with: event) { return }
        super.sendEvent(event)
    }
}

final class Card {
    let row: Record
    let view: Surface
    let close: ActionButton
    let badge: IconBadge
    let header: DragHeader
    let headerRight: CGFloat
    let body: NSTextField
    var link: ActionButton?
    var timer: Timer?
    var projectLabel: DragHeader!
    var info: InfoButton!
    var contentHeight: CGFloat!
    var grouped: Bool?
    init(_ row: Record, open: @escaping () -> Void, openURL: @escaping (URL) -> Void, complete: @escaping (String, String?) -> Void) {
        self.row = row
        if row.kind == "update" {
            body = PlainTextField(wrappingLabelWithString: row.description)
            body.font = .systemFont(ofSize: 12)
            body.textColor = .secondaryLabelColor
            body.maximumNumberOfLines = 2
            body.lineBreakMode = .byWordWrapping
            let measured = (row.description as NSString).boundingRect(with: NSSize(width: 308, height: 1000), options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: [.font: body.font!]).height
            body.frame = NSRect(x: 0, y: 0, width: 312, height: row.description.isEmpty ? 0 : min(34, max(17, ceil(measured) + 2)))
        } else {
            body = label(row.question, width: 312, size: 12, color: .secondaryLabelColor)
        }
        body.toolTip = row.kind == "update" ? row.description : row.question
        let hasAction = row.kind == "update" || row.linkURL != nil
        let actionTitle = row.kind == "update" ? "Read update" : (row.linkLabel ?? "")
        let actionWidth: CGFloat = min(260, max(126, ceil((actionTitle as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 12, weight: .medium)]).width) + 32))
        let actionSpace: CGFloat = hasAction ? 34 : 0
        headerRight = 280
        let height = body.frame.height + 72 + actionSpace
        view = effect(NSRect(x: 0, y: 0, width: 344, height: height))
        view.severity = row.visualSeverity
        contentHeight = height
        body.setFrameOrigin(NSPoint(x: 16, y: 32 + actionSpace))
        view.content.addSubview(body)
        let source = NSTextField(labelWithString: row.sourceLabel)
        source.font = .systemFont(ofSize: 10, weight: .medium)
        source.textColor = .tertiaryLabelColor
        source.lineBreakMode = .byTruncatingTail
        source.frame = NSRect(x: 30, y: 12 + actionSpace, width: 298, height: 14)
        source.toolTip = row.isLocalSource || row.sourceHost == nil ? row.sourceLabel : "Sent from server: \(row.sourceLabel)"
        source.setAccessibilityLabel(source.toolTip!)
        view.content.addSubview(source)
        let sourceIcon = NSImageView(frame: NSRect(x: 16, y: 13 + actionSpace, width: 10, height: 10))
        sourceIcon.image = NSImage(systemSymbolName: row.isLocalSource ? "laptopcomputer" : (row.sourceHost == nil ? "questionmark.circle" : "server.rack"), accessibilityDescription: nil)
        sourceIcon.contentTintColor = .tertiaryLabelColor
        view.content.addSubview(sourceIcon)
        header = DragHeader(labelWithString: (row.title ?? "Untitled"))
        header.font = .systemFont(ofSize: 13, weight: .semibold)
        header.textColor = .labelColor
        header.lineBreakMode = .byTruncatingTail
        header.frame = NSRect(x: 46, y: height - 32, width: headerRight - 46, height: 20)
        header.toolTip = row.heading
        header.setAccessibilityLabel("\(row.visualLabel): \((row.title ?? "Untitled"))")
        view.content.addSubview(header)
        badge = IconBadge(row, frame: NSRect(x: 14, y: height - 36, width: 24, height: 24))
        view.content.addSubview(badge)
        projectLabel = DragHeader(labelWithString: (row.project ?? "Notifications") + " ·")
        projectLabel.font = .systemFont(ofSize: 11, weight: .medium)
        projectLabel.textColor = .secondaryLabelColor
        projectLabel.lineBreakMode = .byTruncatingTail
        projectLabel.toolTip = (row.project ?? "Notifications")
        let projectWidth = min(150, ceil((projectLabel.stringValue as NSString).size(withAttributes: [.font: projectLabel.font!]).width) + 4)
        projectLabel.frame = NSRect(x: 46, y: height - 31, width: projectWidth, height: 18)
        view.content.addSubview(projectLabel)
        close = ActionButton("", frame: NSRect(x: 312, y: height - 36, width: 24, height: 24), style: .quiet) { complete(row.taskID, nil) }
        close.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Dismiss notification")
        close.image?.size = NSSize(width: 9, height: 9)
        close.setAccessibilityLabel("Dismiss \((row.project ?? "Notifications")) notification: \((row.title ?? "Untitled"))")
        view.content.addSubview(close)
        info = InfoButton(row, frame: NSRect(x: 284, y: height - 36, width: 24, height: 24))
        view.content.addSubview(info)
        if hasAction {
            let button = ActionButton(actionTitle, frame: NSRect(x: 328 - actionWidth, y: 10, width: actionWidth, height: 28), style: .secondary) {
                if row.kind == "update" { open() }
                else if let link = row.linkURL, let url = URL(string: link) { openURL(url) }
                if row.commentsEnabled != true { complete(row.taskID, nil) }
            }
            button.font = .systemFont(ofSize: 12, weight: .medium)
            button.controlSize = .regular
            button.frame = NSRect(x: 328 - actionWidth, y: 10, width: actionWidth, height: button.intrinsicContentSize.height)
            button.alignment = .center
            button.cell!.lineBreakMode = .byTruncatingTail
            button.toolTip = row.kind == "update" ? "Read update" : (row.linkLabel ?? "Open")
            button.setAccessibilityLabel("\(row.kind == "update" ? "Read update" : (row.linkLabel ?? "Open")) for \((row.project ?? "Notifications")): \((row.title ?? "Untitled"))")
            view.content.addSubview(button)
            link = button
        }
    }
    func configure(grouped: Bool) {
        if self.grouped == grouped { return }
        self.grouped = grouped
        projectLabel.isHidden = grouped
        header.frame.origin.x = grouped ? 46 : projectLabel.frame.maxX + 5
        header.frame.size.width = max(0, headerRight - header.frame.minX)
        view.drawsSurface = !grouped
        view.isHidden = false
        view.needsDisplay = true
    }
    deinit { timer?.invalidate() }
}

final class ProjectGroup {
    let view = Surface(frame: .zero)
    let project: String
    let count = PlainTextField(labelWithString: "")
    let summary = DragHeader(labelWithString: "")
    let detail = DragHeader(labelWithString: "")
    let toggle: ActionButton
    let clear: ActionButton
    var dividers: [NSBox] = []
    var badge: IconBadge?
    init(_ project: String, toggle: @escaping () -> Void, clear: @escaping () -> Void) {
        self.project = project
        count.font = .monospacedDigitSystemFont(ofSize: 11, weight: .semibold)
        count.textColor = secondaryTextColor()
        count.alignment = .right
        summary.font = .systemFont(ofSize: 13, weight: .semibold)
        summary.textColor = .labelColor
        summary.lineBreakMode = .byTruncatingTail
        detail.font = .systemFont(ofSize: 12)
        detail.textColor = secondaryTextColor()
        detail.lineBreakMode = .byTruncatingTail
        self.toggle = ActionButton(project, frame: .zero, style: .quiet, action: toggle)
        self.toggle.font = .systemFont(ofSize: 11, weight: .medium)
        self.toggle.contentTintColor = .secondaryLabelColor
        self.toggle.alignment = .left
        self.toggle.imagePosition = .imageLeading
        self.toggle.cell!.lineBreakMode = .byTruncatingTail
        self.clear = ActionButton("", frame: .zero, style: .quiet, action: clear)
        self.clear.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Clear project notifications")
        self.clear.toolTip = "Clear all \(project) notifications"
        self.clear.setAccessibilityLabel("Clear all \(project) notifications")
        for child in [count, summary, detail, self.toggle, self.clear] { view.content.addSubview(child) }
    }
    func update(_ cards: [Card], expanded: Bool) {
        let height: CGFloat = expanded ? 30 + cards.reduce(CGFloat(0)) { $0 + $1.view.frame.height + 1 } : 66
        view.setFrameSize(NSSize(width: 344, height: height))
        count.stringValue = "\(cards.count)"
        count.frame = NSRect(x: 276, y: height - 24, width: 24, height: 18)
        toggle.frame = NSRect(x: 8, y: height - 28, width: 264, height: 24)
        toggle.image = NSImage(systemSymbolName: expanded ? "chevron.down" : "chevron.right", accessibilityDescription: expanded ? "Collapse project" : "Expand project")
        toggle.setAccessibilityLabel("\(expanded ? "Collapse" : "Expand") \(project), \(cards.count) notifications")
        toggle.toolTip = "\(expanded ? "Collapse" : "Expand") \(project)"
        clear.setAccessibilityLabel("Clear all \(cards.count) \(project) notifications")
        clear.frame = NSRect(x: 308, y: height - 28, width: 28, height: 24)
        let priority = cards.map { $0.row.visualSeverity.priority }.max() ?? 0
        guard let featured = cards.first(where: { $0.row.visualSeverity.priority == priority })?.row else { return }
        view.severity = expanded ? .neutral : featured.visualSeverity
        badge?.removeFromSuperview()
        let badge = IconBadge(featured, frame: NSRect(x: 12, y: 10, width: 22, height: 22))
        self.badge = badge
        badge.isHidden = expanded
        view.content.addSubview(badge)
        summary.stringValue = featured.visualSeverity == .neutral ? (featured.title ?? "Untitled") : "\(featured.visualLabel) · \((featured.title ?? "Untitled"))"
        summary.toolTip = summary.stringValue
        summary.frame = NSRect(x: 42, y: 22, width: 288, height: 18)
        summary.isHidden = expanded
        let sources = Array(Set(cards.map { $0.row.sourceLabel })).sorted().joined(separator: ", ")
        detail.stringValue = sources + " · " + (featured.kind == "update" ? featured.description : markdown(featured.question, size: 12, color: secondaryTextColor()).string.replacingOccurrences(of: "\n", with: " "))
        detail.toolTip = detail.stringValue
        detail.frame = NSRect(x: 42, y: 6, width: 288, height: 16)
        detail.isHidden = expanded
        while dividers.count > cards.count { dividers.removeLast().removeFromSuperview() }
        while dividers.count < cards.count {
            let divider = NSBox()
            divider.boxType = .separator
            view.content.addSubview(divider)
            dividers.append(divider)
        }
        var y = height - 30
        for (index, card) in cards.enumerated() {
            card.view.isHidden = !expanded
            if card.view.superview !== view.content { view.content.addSubview(card.view) }
            y -= card.view.frame.height + 1
            card.view.setFrameOrigin(NSPoint(x: 0, y: y))
            dividers[index].frame = NSRect(x: 18, y: y + card.view.frame.height, width: 308, height: 1)
            dividers[index].isHidden = !expanded
        }
    }
}

// Historical records can predate request validation. Never pass an unbounded
// or non-finite interval into AppKit's timer even when saved expiry is damaged.
func notificationTimerInterval(autoclose: Double?, expiry: Double?, now: Double = Date().timeIntervalSince1970) -> Double? {
    guard let seconds = autoclose, seconds.isFinite, seconds > 0, seconds <= 31_536_000,
          now.isFinite, now >= 0 else { return nil }
    guard let expiry, expiry.isFinite else { return seconds }
    return max(0.001, min(seconds, expiry - now))
}

final class Interface {
    var coalescesArrivalLayout = false
    var arrivalLayoutPending = false
    var layoutPasses = 0
    func scheduleArrivalLayout() {
        guard coalescesArrivalLayout else { layout(); return }
        guard !arrivalLayoutPending else { return }
        arrivalLayoutPending = true
        onMain { [weak self] in guard let self else { return }; self.arrivalLayoutPending = false; self.layout() }
    }
    let present: Bool
    let stack = panel("Hey Boss notifications")
    let scroll = NSScrollView()
    let stackContent = NSView()
    let stackToolbar = Surface(frame: NSRect(x: 0, y: 0, width: 344, height: 34))
    let notificationCount = DragHeader(labelWithString: "Notifications")
    let closeAll = ActionButton("Close all", frame: NSRect(x: 244, y: 3, width: 88, height: 28), style: .secondary, action: {})
    let hideStack = ActionButton("", frame: NSRect(x: 310, y: 5, width: 28, height: 24), style: .quiet, action: {})
    var stackHiddenByUser = false
    let document = NSView()
    let glassContainer = NSGlassEffectContainerView()
    let question = panel("Hey Boss question")
    var cards: [Card] = []
    var dismissalAnimations = 0
    var groupHeaders: [NSView] = []
    var projectGroups: [String: ProjectGroup] = [:]
    var expandedProjects: Set<String> = []
    var questions: [Record] = []
    var questionDrafts: [String: String] = [:]
    var current: Record?
    var field: GrowingTextInput?
    var buttons: [ActionButton] = []
    var onComplete: (String, String?) -> Void = { _, _ in }
    var onCompleteMany: ([String]) -> Void = { _ in }
    var onDismissMany: ([String]) -> Void = { _ in }
    var onPresented: (String, Double) -> Void = { _, _ in }
    var observer: NSObjectProtocol?
    var saveComment: (String, String, String?, String?, DocumentSelection?, @escaping (Result<Record, Error>) -> Void) -> Void = { _, _, _, _, _, completion in completion(.failure(StorageError(description: "Review storage unavailable"))) }
    var finishReview: (String, @escaping (Result<Record, Error>) -> Void) -> Void = { _, completion in completion(.failure(StorageError(description: "Review storage unavailable"))) }
    var previews: [String: Preview] = [:]
    var openURL: (URL) -> Void = {
        if !NSWorkspace.shared.open($0) { NSLog("Unable to open link: %@", $0.absoluteString) }
    }
    init(present: Bool) {
        self.present = present
        coalescesArrivalLayout = present
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        glassContainer.contentView = document
        glassContainer.spacing = 0
        scroll.documentView = glassContainer
        stack.contentView = stackContent
        stackContent.addSubview(scroll)
        stackContent.addSubview(stackToolbar)
        notificationCount.font = .systemFont(ofSize: 11, weight: .medium)
        notificationCount.textColor = .secondaryLabelColor
        notificationCount.frame = NSRect(x: 14, y: 8, width: 188, height: 18)
        notificationCount.toolTip = "Drag to move notifications"
        stackToolbar.content.addSubview(notificationCount)
        closeAll.controlSize = .regular
        closeAll.font = .systemFont(ofSize: 12, weight: .medium)
        closeAll.frame = NSRect(x: 210, y: 3, width: 88, height: 28)
        closeAll.toolTip = "Dismiss all notifications and cancel unanswered questions; history is kept"
        closeAll.setAccessibilityLabel("Close all notifications and cancel unanswered questions")
        closeAll.invoke = { [weak self] in
            guard let self else { return }
            let ids = self.cards.map { $0.row.taskID } + self.questions.map(\.taskID) + [self.current?.taskID].compactMap { $0 }
            if !ids.isEmpty { self.onDismissMany(ids) }
        }
        stackToolbar.content.addSubview(closeAll)
        hideStack.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Hide notifications")
        hideStack.toolTip = "Hide this window; unread items remain in Inbox"
        hideStack.setAccessibilityLabel("Hide notifications")
        stack.dismissWindow = { [weak self] in
            self?.stackHiddenByUser = true
            self?.stack.orderOut(nil)
        }
        hideStack.invoke = { [weak self] in self?.stack.performClose(nil) }
        stackToolbar.content.addSubview(hideStack)
        stack.hasShadow = false
        observer = NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in self?.layout() }
    }
    func add(_ row: Record) {
        guard !cards.contains(where: { $0.row.taskID == row.taskID }), !questions.contains(where: { $0.taskID == row.taskID }), current?.taskID != row.taskID else { return }
        stackHiddenByUser = false
        if row.kind == "alert" || row.kind == "update" {
            let card = Card(row, open: { [weak self] in self?.openPreview(row) }, openURL: openURL) { [weak self] id, answer in
                if row.commentsEnabled == true { self?.onDismissMany([id]) }
                else { self?.onComplete(id, answer) }
            }
            // A pending dismissal freezes stack geometry. Keep arrivals hidden
            // until the final layout gives them their correct position.
            card.view.isHidden = dismissalAnimations > 0
            cards.append(card)
            document.addSubview(card.view)
            scheduleArrivalLayout()
            let now = Date().timeIntervalSince1970
            onPresented(row.taskID, now)
            if let interval = notificationTimerInterval(autoclose: row.autoclose, expiry: row.expiresAt, now: now) {
                card.timer = Timer.scheduledTimer(withTimeInterval: interval, repeats: false) { [weak self] _ in self?.onComplete(row.taskID, nil) }
            } else if row.autoclose != nil {
                reportFailure(StorageError(description: "Ignored invalid saved autoclose; the card can be dismissed manually"))
            }

        } else {
            questions.append(row)
            nextQuestion()
            scheduleArrivalLayout()
        }
    }
    func openPreview(_ row: Record) {
        if previews[row.taskID] == nil {
            let preview = Preview(row, openURL: openURL)
            preview.saveComment = saveComment
            preview.finishReview = finishReview
            preview.onClose = { [weak self] in self?.previews.removeValue(forKey: row.taskID) }
            previews[row.taskID] = preview
        }
        if present {
            previews[row.taskID]?.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        }
    }
    func finishCardRemoval(_ removed: [Card], animated: Bool? = nil) {
        for card in removed { card.timer?.invalidate() }
        guard animated ?? (present && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion), !removed.isEmpty else {
            for card in removed { card.view.removeFromSuperview() }
            layout()
            return
        }
        dismissalAnimations += 1
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.18
            context.timingFunction = CAMediaTimingFunction(name: .easeOut)
            for card in removed { card.view.animator().alphaValue = 0 }
        }, completionHandler: { [weak self] in
            for card in removed { card.view.removeFromSuperview() }
            guard let self else { return }
            self.dismissalAnimations -= 1
            if self.dismissalAnimations == 0 {
                NSAnimationContext.runAnimationGroup({ context in
                    context.duration = 0.20
                    context.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
                    context.allowsImplicitAnimation = true
                    self.layout(animated: true)
                })
            }
        })
    }
    func remove(_ ids: [String]) {
        let removedIDs = Set(ids)
        for id in ids { questionDrafts.removeValue(forKey: id) }
        let removedCards = cards.filter { removedIDs.contains($0.row.taskID) }
        cards.removeAll { removedIDs.contains($0.row.taskID) }
        questions.removeAll { removedIDs.contains($0.taskID) }
        if let current, removedIDs.contains(current.taskID) {
            self.current = nil
            question.orderOut(nil)
            question.contentView = nil
            buttons = []
            field = nil
            nextQuestion()
        }
        finishCardRemoval(removedCards)
    }
    func remove(_ id: String) { remove([id]) }
    func layout(animated: Bool = false) {
        layoutPasses += 1
        if dismissalAnimations > 0 { return }
        func position(_ view: NSView, _ point: NSPoint) {
            if animated && present { view.animator().setFrameOrigin(point) } else { view.setFrameOrigin(point) }
        }
        let screen = stack.visibleArea
        groupHeaders = []
        let groupedProjects = Set(Dictionary(grouping: cards, by: { ($0.row.project ?? "Notifications") }).filter { $0.value.count >= 3 }.keys)
        for project in Array(projectGroups.keys) where !groupedProjects.contains(project) {
            projectGroups.removeValue(forKey: project)?.view.removeFromSuperview()
        }
        let count = cards.count + questions.count + (current == nil ? 0 : 1)
        if count == 0 { stack.orderOut(nil); return }
        notificationCount.stringValue = "\(count) \(count == 1 ? "notification" : "notifications")"
        let groups = Dictionary(grouping: cards, by: { ($0.row.project ?? "Notifications") }).map { project, cards in
            (project: project, cards: cards.sorted { $0.row.createdAt > $1.row.createdAt })
        }.sorted {
            if $0.cards[0].row.createdAt == $1.cards[0].row.createdAt { return $0.project < $1.project }
            return $0.cards[0].row.createdAt > $1.cards[0].row.createdAt
        }
        for group in groups {
            for card in group.cards { card.configure(grouped: group.cards.count >= 3) }
            if group.cards.count < 3 { expandedProjects.remove(group.project) }
        }
        let total = groups.reduce(CGFloat(0)) { total, group in
            if group.cards.count < 3 {
                return total + group.cards.reduce(CGFloat(0)) { $0 + $1.view.frame.height + 8 }
            }
            return total + (expandedProjects.contains(group.project) ? 30 + group.cards.reduce(CGFloat(0)) { $0 + $1.view.frame.height + 1 } : 66) + 8
        }
        let toolbarHeight: CGFloat = 42
        let height = min(total, screen.height - 24 - toolbarHeight)
        let gutter = total > height && scroll.scrollerStyle == .legacy ? NSScroller.scrollerWidth(for: .regular, scrollerStyle: .legacy) : 0
        let oldTop = document.frame.height - scroll.contentView.bounds.maxY
        stack.place(NSRect(x: screen.minX + 12, y: screen.maxY - height - toolbarHeight - 12, width: 344 + gutter, height: height + toolbarHeight), display: present, animated: animated)
        stackToolbar.frame = NSRect(x: 0, y: height + 8, width: 344, height: 34)
        scroll.frame = NSRect(x: 0, y: 0, width: 344 + gutter, height: height)
        glassContainer.frame = NSRect(x: 0, y: 0, width: 344, height: total)
        document.frame = glassContainer.bounds
        var y = total
        for group in groups {
            if group.cards.count < 3 {
                for card in group.cards {
                    if card.view.superview !== document { document.addSubview(card.view) }
                    y -= card.view.frame.height
                    position(card.view, NSPoint(x: 0, y: y))
                    y -= 8
                }
                continue
            }
            let expanded = expandedProjects.contains(group.project)
            let project = group.project
            if projectGroups[project] == nil {
                projectGroups[project] = ProjectGroup(project, toggle: { [weak self] in
                    guard let self else { return }
                    if self.expandedProjects.contains(project) { self.expandedProjects.remove(project) }
                    else { self.expandedProjects.insert(project) }
                    self.layout()
                }, clear: { [weak self] in
                    guard let self else { return }
                    self.onDismissMany(self.cards.filter { ($0.row.project ?? "Notifications") == project }.map { $0.row.taskID })
                })
            }
            guard let projectView = projectGroups[group.project] else { continue }
            projectView.update(group.cards, expanded: expanded)
            y -= projectView.view.frame.height
            if projectView.view.superview !== document { document.addSubview(projectView.view) }
            position(projectView.view, NSPoint(x: 0, y: y))
            groupHeaders.append(projectView.view)
            y -= 8
        }
        scroll.contentView.scroll(to: NSPoint(x: 0, y: max(0, total - height - max(0, oldTop))))
        scroll.reflectScrolledClipView(scroll.contentView)
        if present && !stackHiddenByUser { stack.orderFrontRegardless() }
    }
    func presentQuestion(_ row: Record) {
        add(row)
        if current?.taskID != row.taskID {
            if let current {
                if let field { questionDrafts[current.taskID] = field.stringValue }
                questions.insert(current, at: 0)
            }
            current = nil
            questions.removeAll { $0.taskID == row.taskID }
            questions.insert(row, at: 0)
            nextQuestion()
        }
        if present { question.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true) }
    }
    func nextQuestion() {
        if current != nil || questions.isEmpty { return }
        let row = questions.removeFirst()
        current = row
        question.title = row.heading
        buttons = []
        field = nil
        let title = label(row.question, width: 432, size: 15, color: .labelColor)
        let description = label(row.description, width: 432, size: 12, color: secondaryTextColor())
        if row.description.isEmpty { description.frame.size.height = 0 }
        let optionWidths = row.options.map { max(88, ($0 as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 13, weight: .medium)]).width + 32) }
        let optionsWidth = optionWidths.reduce(0, +) + CGFloat(max(0, row.options.count - 1)) * 8
        let inlineOptions = optionsWidth <= 432
        let optionHeights = row.options.map {
            max(44, ceil(($0 as NSString).boundingRect(with: NSSize(width: 384, height: CGFloat.greatestFiniteMagnitude), options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: [.font: NSFont.systemFont(ofSize: 13, weight: .medium)]).height) + 20)
        }
        let inputHeight: CGFloat = row.kind == "prompt" ? 120 : inlineOptions ? 48 : optionHeights.reduce(CGFloat(0)) { $0 + $1 + 8 }
        let natural = 68 + title.frame.height + description.frame.height + inputHeight
        let screen = question.visibleArea
        let height = min(natural, screen.height - 80)
        let view = effect(NSRect(x: 0, y: 0, width: 480, height: height))
        let body = NSView(frame: NSRect(x: 0, y: 0, width: 480, height: natural))
        let bodyScroll = NSScrollView(frame: view.bounds)
        bodyScroll.drawsBackground = false
        bodyScroll.hasVerticalScroller = true
        bodyScroll.scrollerStyle = .overlay
        bodyScroll.autohidesScrollers = true
        bodyScroll.documentView = body
        view.content.addSubview(bodyScroll)
        let header = DragHeader(labelWithString: row.heading)
        header.toolTip = row.heading
        header.lineBreakMode = .byTruncatingTail
        header.font = .systemFont(ofSize: 11, weight: .medium)
        header.textColor = secondaryTextColor()
        header.frame = NSRect(x: 54, y: natural - 26, width: 368, height: 18)
        header.setAccessibilityLabel("\(row.visualLabel): \(row.heading)")
        body.addSubview(header)
        body.addSubview(IconBadge(row, frame: NSRect(x: 24, y: natural - 29, width: 22, height: 22)))
        body.addSubview(InfoButton(row, frame: NSRect(x: 430, y: natural - 30, width: 26, height: 24)))
        title.setFrameOrigin(NSPoint(x: 24, y: natural - 36 - title.frame.height))
        body.addSubview(title)
        description.setFrameOrigin(NSPoint(x: 24, y: title.frame.minY - 6 - description.frame.height))
        body.addSubview(description)
        if row.kind == "prompt" {
            let input = GrowingTextInput(frame: NSRect(x: 24, y: 72, width: 432, height: 64))
            input.text.setAccessibilityLabel(row.question)
            body.addSubview(input); field = input
            let submit = ActionButton("Submit", frame: NSRect(x: 350, y: 22, width: 106, height: 40), style: .primary) { [weak self, weak input] in guard let input else { return }; self?.answer(input.stringValue) }
            submit.keyEquivalent = "\r"; submit.keyEquivalentModifierMask = .command
            input.submit = { [weak self, weak input] in guard let input else { return }; self?.answer(input.stringValue) }
            input.heightChanged = { [weak self, weak input, weak body, weak bodyScroll] height in
                guard let self, let input, let body, let bodyScroll else { return }
                let delta = height - input.frame.height
                guard abs(delta) > 0.5 else { return }
                input.frame.size.height = height; body.frame.size.height += delta
                for child in body.subviews where child !== input && child !== submit { child.frame.origin.y += delta }
                let nextHeight = min(body.frame.height, screen.height - 80)
                var rect = self.question.frame; rect.origin.y -= (nextHeight - rect.height) / 2; rect.size.height = nextHeight
                self.question.place(rect, display: self.present)
                bodyScroll.frame.size.height = nextHeight
                input.resizeEditor()
            }
            input.stringValue = questionDrafts[row.taskID] ?? ""
            body.addSubview(submit); buttons = [submit]
        } else {
            var x = 456 - optionsWidth
            var optionY = 20 + inputHeight
            for (index, option) in row.options.enumerated() {
                optionY -= optionHeights[index] + 8
                let frame = inlineOptions ? NSRect(x: x, y: 22, width: optionWidths[index], height: 44) : NSRect(x: 24, y: optionY, width: 432, height: optionHeights[index])
                let button = ActionButton(option, frame: frame, style: .secondary) { [weak self] in self?.answer(option) }
                button.toolTip = option
                if !inlineOptions {
                    button.bezelStyle = .flexiblePush
                    button.borderShape = .roundedRectangle
                    button.controlSize = .large
                    button.alignment = .left
                    button.cell!.wraps = true
                    button.cell!.usesSingleLineMode = false
                    button.cell!.lineBreakMode = .byWordWrapping
                    button.frame = frame
                }
                body.addSubview(button)
                buttons.append(button)
                x += optionWidths[index] + 8
            }
        }
        let finalHeight = min(body.frame.height, screen.height - 80)
        view.frame.size.height = finalHeight; bodyScroll.frame = view.bounds
        question.contentView = view
        question.place(NSRect(x: screen.midX - 240, y: screen.midY - finalHeight / 2, width: 480, height: finalHeight), display: present)
        bodyScroll.contentView.scroll(to: NSPoint(x: 0, y: max(0, body.frame.height - finalHeight)))
        bodyScroll.reflectScrolledClipView(bodyScroll.contentView)
        if present {
            question.makeKeyAndOrderFront(nil)
            if let field { question.makeFirstResponder(field.text) }
        }
        onPresented(row.taskID, Date().timeIntervalSince1970)
    }
    func retryAnswer(_ id: String, message: String) {
        guard current?.taskID == id else { return }
        for button in buttons { button.isEnabled = true }; field?.isEnabled = true
        if present { let alert = NSAlert(); alert.messageText = "Answer not sent"; alert.informativeText = message; alert.addButton(withTitle: "OK"); alert.beginSheetModal(for: question) }
    }
    func answer(_ text: String) {
        guard let current else { return }
        for button in buttons { button.isEnabled = false }
        field?.isEnabled = false
        onComplete(current.taskID, text)
    }
}

struct AgentInfo: Codable, Equatable {
    let id: String
    let pid: UInt32
    let kind: String
    let cwd: String?
    let sessionId: String?
    let task: String?
    var title: String? = nil
    let activity: String?
    let state: String
    let updatedAt: Double?
    let evidence: String
    let update: String?
    let activityAt: Double?
    let git: AgentGit?
}
struct AgentGit: Codable, Equatable {
    let repositoryRoot: String
    let commonDir: String
    let worktree: String
    let branch: String?
    let repositoryId: String
    let origin: String?
}
struct AgentSnapshot: Codable {
    let host: String
    let observedAt: Double
    let agents: [AgentInfo]
    let warnings: [String]
    static func decode(_ data: Data, allowClockSkew: Bool = false) -> AgentSnapshot? {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        guard let snapshot = try? decoder.decode(AgentSnapshot.self, from: data),
              snapshot.observedAt.isFinite, snapshot.observedAt >= 0,
              (allowClockSkew || snapshot.observedAt <= Date().timeIntervalSince1970 + 300) else { return nil }
        return snapshot
    }
}
// Read only the host inventory; upstream credentials never leave hey-proxy's config.
func overviewSSHHosts(_ data: Data, excluding connectedHost: String?) -> [String] {
    guard data.count <= 1024 * 1024,
          let config = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let entries = config["ssh_hosts"] as? [Any] else { return [] }
    var hosts: [String] = []
    for entry in entries {
        if (entry as? [String: Any])?["enabled"] as? Bool == false { continue }
        guard let host = (entry as? String) ?? (entry as? [String: Any])?["host"] as? String,
              !host.isEmpty, host.count <= 253, !host.hasPrefix("-"),
              !host.contains(where: { $0.isWhitespace || $0.isNewline || $0.asciiValue == 0 }),
              host != connectedHost, !hosts.contains(host) else { continue }
        hosts.append(host)
        if hosts.count == 32 { break }
    }
    return hosts
}
func overviewRetryDelay(_ failures: Int) -> TimeInterval {
    min(900, 60 * pow(2, Double(min(4, max(0, failures - 1)))))
}
func agentRelativeTime(_ timestamp: Double?) -> String? {
    guard let timestamp, timestamp.isFinite, timestamp >= 0 else { return nil }
    let now = Date().timeIntervalSince1970
    if timestamp > now + 300 { return "Clock ahead" }
    let age = Int(max(0, now - timestamp))
    if age < 60 { return "\(age)s ago" }
    if age < 3600 { return "\(age / 60)m ago" }
    if age < 86400 { return "\(age / 3600)h ago" }
    return "\(age / 86400)d ago"
}
func agentUpdatedTime(_ timestamp: Double?) -> String {
    guard let timestamp, timestamp.isFinite, timestamp >= 0,
          timestamp <= Date().timeIntervalSince1970 + 300 else { return "unavailable" }
    return Date(timeIntervalSince1970: timestamp).formatted(date: .abbreviated, time: .standard)
}

// Display paths can belong to a remote host. Never stat them on the UI thread.
func agentPathName(_ path: String) -> String {
    path.split(separator: "/", omittingEmptySubsequences: true).last.map(String.init) ?? path
}
struct OverviewRow: Encodable, Equatable {
    let agent: AgentInfo
    let host: String
    let local: Bool
    let stale: Bool
    var key: String { "\(host):\(agent.id)" }
    var project: String { agent.cwd.map(agentPathName).flatMap { $0.isEmpty ? nil : $0 } ?? "Unknown project" }
    enum CodingKeys: String, CodingKey { case agent, host, local, stale, unattributed, stateLabel }
    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(agent, forKey: .agent)
        try values.encode(host, forKey: .host)
        try values.encode(local, forKey: .local)
        try values.encode(stale, forKey: .stale)
        try values.encode(unattributed, forKey: .unattributed)
        try values.encode(stateLabel, forKey: .stateLabel)
    }

    var unattributed: Bool {
        agent.sessionId == nil && [agent.task, agent.activity, agent.update].allSatisfy { $0?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty != false }
    }
    var chatLabel: String { agent.title ?? taskLabel }
    var taskLabel: String { agent.task ?? (unattributed ? "Unattributed process" : "No task recorded") }
    var stateLabel: String { stale ? (local ? "Discovery stale" : "Offline / stale") : (unattributed ? "No session details" : agent.state) }
}

struct OverviewState: Encodable {
    let observedAt: Double
    let grouping: String
    let search: String
    let filter: String
    let summary: String
    let connection: String
    let scanning: Bool
    let scanError: String?
    let local: AgentSnapshot?
    let servers: [AgentSnapshot]
    let rows: [OverviewRow]
    let collapsedGroups: [String]
    let expandedAgents: [String]
    let performance: [String: Double]
    let selectedRow: String?
}

enum OverviewItem: Equatable {
    case group(key: String, title: String, subtitle: String, count: Int)
    case agent(OverviewRow)
}
final class OverviewDisclosureButton: NSButton {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}
final class OverviewCanvas: NSView {
    override func draw(_ dirtyRect: NSRect) { NSColor.windowBackgroundColor.setFill(); dirtyRect.fill() }
}

final class AgentDetailView: NSTextView {
    var stringValue: String {
        get { string }
        set { string = newValue }
    }
}

final class OverviewCell: NSTableCellView {
    var rowButtons: [NSButton] = []
    var labels: [(NSTextField, NSColor, NSAttributedString)] = []
    func track(_ field: NSTextField) {
        labels.append((field, field.textColor ?? .labelColor, field.attributedStringValue))
        applySelection()
    }
    override var backgroundStyle: NSView.BackgroundStyle { didSet { applySelection() } }
    private func applySelection() {
        for (field, color, original) in labels {
            if backgroundStyle == .emphasized {
                let selected = NSMutableAttributedString(attributedString: original)
                selected.addAttribute(.foregroundColor, value: NSColor.alternateSelectedControlTextColor, range: NSRange(location: 0, length: selected.length))
                field.attributedStringValue = selected
                field.textColor = .alternateSelectedControlTextColor
            } else {
                field.attributedStringValue = original
                field.textColor = color
            }
        }
    }
}

struct ConnectionPreferences: Codable {
    var host: String
    var vpnDomain: String
    var enabled: Bool
    static func load() -> ConnectionPreferences? {
        let path = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/share/hey-boss/companion.json")
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        guard let data = try? Data(contentsOf: path) else { return nil }
        return try? decoder.decode(ConnectionPreferences.self, from: data)
    }
    var validationError: String? {
        if host.isEmpty || host.hasPrefix("-") || host.contains(where: { $0.isWhitespace }) { return "Enter an SSH host or alias, such as devbox or user@host." }
        let domain = vpnDomain.hasSuffix(".") ? String(vpnDomain.dropLast()) : vpnDomain
        let labels = domain.split(separator: ".", omittingEmptySubsequences: false)
        if domain.isEmpty || domain.utf8.count > 253 || labels.contains(where: { label in
            label.isEmpty || label.utf8.count > 63 || label.first == "-" || label.last == "-" || !label.utf8.allSatisfy { byte in (48...57).contains(byte) || (65...90).contains(byte) || (97...122).contains(byte) || byte == 45 }
        }) { return "Enter a VPN DNS domain, such as quora.net." }
        return nil
    }
}

func connectionStateDescription(_ state: [String: Any]?, host: String) -> String {
    switch state?["state"] as? String {
    case "connected": return "Connected to \(host)"
    case "connecting": return "Connecting to \(host)…"
    case "waiting-for-vpn": return "Waiting for VPN"
    case "waiting-for-stable-vpn": return "VPN detected · waiting for a stable connection"
    case "backoff":
        if let retry = state?["retry_at"] as? Double, retry.isFinite, retry >= Date().timeIntervalSince1970, retry <= Date().timeIntervalSince1970 + 86400 {
            return "Connection unavailable · retrying at \(Date(timeIntervalSince1970: retry).formatted(date: .omitted, time: .shortened))"
        }
        return "Connection unavailable · waiting to retry"
    case "config-error": return "Connection settings are invalid · repair or save settings to retry"
    case "disabled": return "Automatic connection off"
    case "stopped": return "Automatic connection paused"
    default: return "Automatic connection not configured"
    }
}

final class ConnectionSettingsController: NSObject, NSWindowDelegate {
    let window: NSWindow
    let host = NSTextField(string: "")
    let vpnDomain = NSTextField(string: "")
    let automatic = NSButton(checkboxWithTitle: "Connect automatically when VPN is available", target: nil, action: nil)
    let feedback = NSTextField(wrappingLabelWithString: "")
    let statusLabel = NSTextField(labelWithString: "")
    let saveButton = NSButton(title: "Save", target: nil, action: nil)
    let cancelButton = NSButton(title: "Cancel", target: nil, action: nil)
    let preview: Bool
    var saving = false
    var saveGeneration = 0
    var onClose: () -> Void = {}
    let apply: (ConnectionPreferences, @escaping (String?) -> Void) -> Void
    init(preferences: ConnectionPreferences, status: String, preview: Bool = false, apply: @escaping (ConnectionPreferences, @escaping (String?) -> Void) -> Void) {
        self.preview = preview
        self.apply = apply
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 580, height: 380), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        super.init()
        window.title = "Server connection"
        window.isReleasedWhenClosed = false
        window.delegate = self
        let content = OverviewCanvas(frame: NSRect(x: 0, y: 0, width: 580, height: 380))
        window.contentView = content
        let title = NSTextField(labelWithString: "Server connection")
        title.font = .systemFont(ofSize: 20, weight: .semibold)
        let introduction = NSTextField(wrappingLabelWithString: "Receive queued notifications and share agent activity whenever your server is connected.")
        introduction.font = .systemFont(ofSize: 13)
        introduction.textColor = .secondaryLabelColor
        let hostLabel = NSTextField(labelWithString: "SSH host")
        let vpnLabel = NSTextField(labelWithString: "VPN domain")
        for label in [hostLabel, vpnLabel] { label.font = .systemFont(ofSize: 13); label.alignment = .right }
        host.stringValue = preferences.host
        vpnDomain.stringValue = preferences.vpnDomain
        host.placeholderString = "devbox or user@host"
        vpnDomain.placeholderString = "quora.net"
        for field in [host, vpnDomain] { field.font = .systemFont(ofSize: 13); field.bezelStyle = .roundedBezel }
        host.setAccessibilityLabel("SSH host")
        vpnDomain.setAccessibilityLabel("VPN domain")
        automatic.state = preferences.enabled ? .on : .off
        let explanation = NSTextField(wrappingLabelWithString: "Waits for VPN DNS before connecting with SSH. Retries slow down after connection failures.")
        explanation.font = .systemFont(ofSize: 12)
        explanation.textColor = .secondaryLabelColor
        let currentStatus = statusLabel
        currentStatus.stringValue = status
        currentStatus.font = .systemFont(ofSize: 12, weight: .medium)
        currentStatus.textColor = .secondaryLabelColor
        currentStatus.lineBreakMode = .byTruncatingTail
        feedback.font = .systemFont(ofSize: 12)
        feedback.maximumNumberOfLines = 2
        feedback.textColor = .secondaryLabelColor
        if preview { feedback.stringValue = "Preview · settings are not applied." }
        saveButton.target = self; saveButton.action = #selector(save)
        saveButton.keyEquivalent = "\r"
        cancelButton.target = self; cancelButton.action = #selector(cancel)
        cancelButton.keyEquivalent = "\u{1b}"
        for view in [title, introduction, hostLabel, host, vpnLabel, vpnDomain, automatic, explanation, currentStatus, feedback, saveButton, cancelButton] {
            view.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(view)
        }
        NSLayoutConstraint.activate([
            title.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 24), title.topAnchor.constraint(equalTo: content.topAnchor, constant: 20),
            introduction.leadingAnchor.constraint(equalTo: title.leadingAnchor), introduction.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), introduction.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 8),
            hostLabel.leadingAnchor.constraint(equalTo: title.leadingAnchor), hostLabel.widthAnchor.constraint(equalToConstant: 92), hostLabel.centerYAnchor.constraint(equalTo: host.centerYAnchor),
            host.leadingAnchor.constraint(equalTo: hostLabel.trailingAnchor, constant: 16), host.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), host.topAnchor.constraint(equalTo: introduction.bottomAnchor, constant: 22), host.heightAnchor.constraint(equalToConstant: 26),
            vpnLabel.leadingAnchor.constraint(equalTo: hostLabel.leadingAnchor), vpnLabel.widthAnchor.constraint(equalTo: hostLabel.widthAnchor), vpnLabel.centerYAnchor.constraint(equalTo: vpnDomain.centerYAnchor),
            vpnDomain.leadingAnchor.constraint(equalTo: host.leadingAnchor), vpnDomain.trailingAnchor.constraint(equalTo: host.trailingAnchor), vpnDomain.topAnchor.constraint(equalTo: host.bottomAnchor, constant: 16), vpnDomain.heightAnchor.constraint(equalToConstant: 26),
            automatic.leadingAnchor.constraint(equalTo: host.leadingAnchor), automatic.topAnchor.constraint(equalTo: vpnDomain.bottomAnchor, constant: 18), automatic.trailingAnchor.constraint(lessThanOrEqualTo: host.trailingAnchor),
            explanation.leadingAnchor.constraint(equalTo: automatic.leadingAnchor), explanation.trailingAnchor.constraint(equalTo: host.trailingAnchor), explanation.topAnchor.constraint(equalTo: automatic.bottomAnchor, constant: 7),
            currentStatus.leadingAnchor.constraint(equalTo: title.leadingAnchor), currentStatus.trailingAnchor.constraint(equalTo: introduction.trailingAnchor), currentStatus.topAnchor.constraint(equalTo: explanation.bottomAnchor, constant: 18),
            feedback.leadingAnchor.constraint(equalTo: title.leadingAnchor), feedback.trailingAnchor.constraint(equalTo: introduction.trailingAnchor), feedback.topAnchor.constraint(equalTo: currentStatus.bottomAnchor, constant: 7),
            saveButton.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), saveButton.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -18), saveButton.widthAnchor.constraint(greaterThanOrEqualToConstant: 76),
            cancelButton.trailingAnchor.constraint(equalTo: saveButton.leadingAnchor, constant: -10), cancelButton.centerYAnchor.constraint(equalTo: saveButton.centerYAnchor)
        ])
        window.initialFirstResponder = host
    }
    func show(on parent: NSWindow) { parent.beginSheet(window) }
    @objc func save() {
        guard !saving else { return }
        let preferences = ConnectionPreferences(host: host.stringValue.trimmingCharacters(in: .whitespacesAndNewlines), vpnDomain: vpnDomain.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).lowercased(), enabled: automatic.state == .on)
        if let error = preferences.validationError { feedback.stringValue = error; feedback.textColor = .systemRed; return }
        saving = true
        saveGeneration += 1
        let generation = saveGeneration
        saveButton.isEnabled = false; cancelButton.isEnabled = false
        host.isEnabled = false; vpnDomain.isEnabled = false; automatic.isEnabled = false
        feedback.stringValue = "Saving…"; feedback.textColor = .secondaryLabelColor
        apply(preferences) { [weak self] error in self?.receiveSave(error, generation: generation) }
    }
    func receiveSave(_ error: String?, generation: Int) {
        if !Thread.isMainThread { onMain { [weak self] in self?.receiveSave(error, generation: generation) }; return }
        guard saving, generation == saveGeneration else { return }
        saving = false
        saveButton.isEnabled = true; cancelButton.isEnabled = true
        host.isEnabled = true; vpnDomain.isEnabled = true; automatic.isEnabled = true
        if let error { feedback.stringValue = error; feedback.textColor = .systemRed }
        else if preview { feedback.stringValue = "Preview settings validated · no changes applied."; feedback.textColor = .secondaryLabelColor }
        else { cancel() }
    }
    @objc func cancel() {
        guard !saving else { return }
        if let parent = window.sheetParent { parent.endSheet(window) }
        window.orderOut(nil)
        onClose()
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool { cancel(); return false }
}

// Shared wrapping editor: long uninterrupted values wrap, grow, then scroll.
final class GrowingTextInput: NSScrollView, NSTextViewDelegate {
    let text = NSTextView()
    var changed: (() -> Void)?
    var submit: (() -> Void)?
    var heightChanged: ((CGFloat) -> Void)?
    var stringValue: String { get { text.string } set { text.string = newValue; resizeEditor() } }
    var isEnabled: Bool { get { text.isEditable } set { text.isEditable = newValue } }
    override init(frame: NSRect) {
        super.init(frame: frame)
        borderType = .bezelBorder; hasVerticalScroller = true; autohidesScrollers = true
        drawsBackground = true; backgroundColor = .textBackgroundColor
        text.isRichText = false; text.importsGraphics = false; text.font = .monospacedSystemFont(ofSize: 14, weight: .regular)
        text.isAutomaticQuoteSubstitutionEnabled = false; text.isAutomaticDashSubstitutionEnabled = false
        text.isAutomaticTextReplacementEnabled = false; text.isAutomaticSpellingCorrectionEnabled = false
        text.isContinuousSpellCheckingEnabled = false; text.isGrammarCheckingEnabled = false
        text.isAutomaticLinkDetectionEnabled = false; text.isAutomaticDataDetectionEnabled = false
        text.allowsUndo = true; text.textContainerInset = NSSize(width: 8, height: 8)
        text.minSize = .zero; text.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        text.isVerticallyResizable = true; text.isHorizontallyResizable = false
        text.autoresizingMask = [.width]; text.textContainer?.widthTracksTextView = true
        text.textContainer?.lineBreakMode = .byCharWrapping
        text.delegate = self; documentView = text; resizeEditor()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    func textDidChange(_ notification: Notification) { resizeEditor(); changed?() }
    func textView(_ view: NSTextView, doCommandBy selector: Selector) -> Bool {
        if selector == #selector(NSResponder.insertNewline(_:)), NSApp.currentEvent?.modifierFlags.contains(.command) == true { submit?(); return true }
        return false
    }
    func resizeEditor() {
        text.frame.size.width = max(40, contentSize.width)
        text.textContainer?.containerSize = NSSize(width: text.frame.width - 16, height: .greatestFiniteMagnitude)
        guard let container = text.textContainer, let manager = text.layoutManager else { return }
        manager.ensureLayout(for: container)
        let used = ceil(manager.usedRect(for: container).height) + 20
        text.frame.size.height = max(contentSize.height, used)
        heightChanged?(min(220, max(64, used)))
    }
    func clear() { text.string = ""; text.undoManager?.removeAllActions() }
}

struct SecretForm: Decodable {
    var fields: [String]
    var login: Bool
    var destination: String
    static func decode(_ request: Request) -> SecretForm? {
        guard request.sync, let project = request.project, !project.isEmpty, project.utf8.count <= 200,
              let title = request.title, !title.isEmpty, title.utf8.count <= 300,
              let raw = request.question, let value = try? JSONDecoder().decode(Self.self, from: Data(raw.utf8)),
              (1...2).contains(value.fields.count), Set(value.fields).count == value.fields.count,
              !value.login || value.fields.count == 2, value.destination.utf8.count <= 2048,
              value.fields.allSatisfy({ $0.range(of: "^[A-Za-z_][A-Za-z0-9_]{0,127}$", options: .regularExpression) != nil }) else { return nil }
        return value
    }
}
final class SecretEntry: NSObject, NSTextFieldDelegate {
    let view = NSStackView()
    let masked = NSSecureTextField()
    let revealed = GrowingTextInput(frame: NSRect(x: 0, y: 0, width: 590, height: 64))
    let toggle = NSButton(checkboxWithTitle: "Show", target: nil, action: nil)
    let count = NSTextField(labelWithString: "0 characters")
    var maskedMode: Bool
    let editorHeight: NSLayoutConstraint
    var changed: (() -> Void)?
    var value: String { maskedMode ? masked.stringValue : revealed.stringValue }
    init(name: String, secret: Bool) {
        maskedMode = secret
        editorHeight = revealed.heightAnchor.constraint(equalToConstant: 64)
        super.init()
        view.orientation = .vertical; view.alignment = .leading; view.spacing = 8
        let header = NSStackView(views: [NSTextField(labelWithString: name), toggle, count]); header.spacing = 12
        count.textColor = .secondaryLabelColor; count.font = .systemFont(ofSize: 11)
        masked.font = .monospacedSystemFont(ofSize: 14, weight: .regular)
        masked.usesSingleLineMode = true; masked.cell?.isScrollable = true; masked.lineBreakMode = .byClipping
        masked.delegate = self; masked.setAccessibilityLabel(name)
        revealed.text.setAccessibilityLabel(name)
        revealed.text.allowsUndo = false
        toggle.target = self; toggle.action = #selector(toggleVisibility); toggle.isHidden = !secret
        view.addArrangedSubview(header); view.addArrangedSubview(masked); view.addArrangedSubview(revealed)
        masked.translatesAutoresizingMaskIntoConstraints = false; revealed.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([masked.widthAnchor.constraint(equalTo: view.widthAnchor), revealed.widthAnchor.constraint(equalTo: view.widthAnchor), masked.heightAnchor.constraint(equalToConstant: 36), editorHeight])
        masked.isHidden = !secret; revealed.isHidden = secret
        revealed.heightChanged = { [weak self] height in self?.editorHeight.constant = height }
        revealed.changed = { [weak self] in self?.updateCount() }
    }
    @objc func toggleVisibility() {
        let current = value; maskedMode = toggle.state != .on
        if maskedMode { masked.stringValue = current; revealed.clear() }
        else { revealed.stringValue = current; masked.stringValue = "" }
        masked.isHidden = !maskedMode; revealed.isHidden = maskedMode
        view.window?.makeFirstResponder(maskedMode ? masked : revealed.text)
        updateCount()
    }
    func updateCount() { count.stringValue = "\(value.count) characters"; changed?() }
    func controlTextDidChange(_ obj: Notification) { updateCount() }
    func clear() { masked.stringValue = ""; revealed.clear(); count.stringValue = "0 characters" }
}

final class SecretFormStack: NSStackView { override var isFlipped: Bool { true } }
final class SecretFormClip: NSClipView { override var isFlipped: Bool { true } }
final class SecretPrompt: NSObject, NSWindowDelegate {
    let window = MachineHealthWindow(contentRect: NSRect(x: 0, y: 0, width: 680, height: 560), styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
    let entries: [SecretEntry]
    let submit = NSButton(title: "Use credentials", target: nil, action: nil)
    let message = NSTextField(wrappingLabelWithString: "")
    var completion: (([String]?) -> Void)?
    var timer: Timer?
    init(request: Request, form: SecretForm, completion: @escaping ([String]?) -> Void) {
        entries = form.fields.enumerated().map { SecretEntry(name: $0.element, secret: !(form.login && $0.offset == 0)) }
        self.completion = completion
        super.init()
        window.title = "\(request.project ?? "Hey Boss") · \(request.title ?? "Credentials")"
        window.isReleasedWhenClosed = false; window.delegate = self
        window.sharingType = .none; window.minSize = NSSize(width: 600, height: 420)
        let content = window.contentView!
        let heading = NSTextField(wrappingLabelWithString: request.title ?? "Enter credentials"); heading.font = .systemFont(ofSize: 22, weight: .semibold)
        let destination = NSTextField(wrappingLabelWithString: form.destination + "\nRequested on " + (request.source_host ?? "This Mac"))
        let note = NSTextField(wrappingLabelWithString: "Sent once to the requesting command. Not saved in Hey Boss history or sent to the mobile app. Show reveals a value only in this window.")
        note.textColor = .secondaryLabelColor; note.font = .systemFont(ofSize: 12)
        let stack = SecretFormStack(views: [heading, destination, note] + entries.map(\.view) + [message]); stack.orientation = .vertical; stack.alignment = .leading; stack.spacing = 16
        let scroll = NSScrollView(); scroll.contentView = SecretFormClip(); scroll.documentView = stack; scroll.hasVerticalScroller = true; scroll.drawsBackground = false
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancel)); cancel.keyEquivalent = "\u{1b}"
        submit.target = self; submit.action = #selector(finish); submit.keyEquivalent = "\r"; submit.keyEquivalentModifierMask = .command
        for v in [scroll, submit, cancel] { v.translatesAutoresizingMaskIntoConstraints = false; content.addSubview(v) }
        stack.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: content.topAnchor, constant: 24), scroll.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 24), scroll.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), scroll.bottomAnchor.constraint(equalTo: submit.topAnchor, constant: -20),
            stack.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor), stack.topAnchor.constraint(equalTo: scroll.contentView.topAnchor), stack.widthAnchor.constraint(equalTo: scroll.widthAnchor, constant: -16),
            submit.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -20), submit.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), cancel.trailingAnchor.constraint(equalTo: submit.leadingAnchor, constant: -12), cancel.centerYAnchor.constraint(equalTo: submit.centerYAnchor)
        ])
        for entry in entries { entry.view.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true; entry.changed = { [weak self] in self?.validate() }; entry.revealed.submit = { [weak self] in self?.finish() } }
        validate()
    }
    func validate() {
        submit.isEnabled = entries.allSatisfy { !$0.value.isEmpty && $0.value.utf8.count <= 65536 && !$0.value.contains("\0") }
        message.stringValue = entries.contains { $0.value.utf8.count > 65536 || $0.value.contains("\0") } ? "Each value must be at most 64 KiB and cannot contain NUL." : "⌘Return submits. Cancel or closing discards all values."
    }
    func show() { NSApp.setActivationPolicy(.regular); window.center(); window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true); if let first = entries.first { window.makeFirstResponder(first.maskedMode ? first.masked : first.revealed.text) } }
    @objc func finish() { validate(); guard submit.isEnabled else { return }; complete(entries.map(\.value)) }
    @objc func cancel() { complete(nil) }
    func windowWillClose(_ notification: Notification) { complete(nil) }
    func complete(_ values: [String]?) {
        guard let done = completion else { return }; completion = nil; timer?.invalidate(); timer = nil
        window.makeFirstResponder(nil); entries.forEach { $0.clear() }; window.orderOut(nil); done(values)
        if !NSApp.windows.contains(where: { $0.isVisible && $0.styleMask.contains(.titled) }) { NSApp.setActivationPolicy(.accessory) }
    }
}
final class SecretPrompts {
    var active: [UUID: SecretPrompt] = [:]
    let present: Bool
    init(present: Bool = true) { self.present = present }
    func handle(_ request: Request, _ reply: Reply) {
        guard let form = SecretForm.decode(request), active.count < 4 else { reply.send(["status":"error", "error":"Invalid secret request or too many open prompts"]); return }
        let id = UUID()
        let prompt = SecretPrompt(request: request, form: form) { [weak self] values in
            self?.active.removeValue(forKey: id)
            if let values, let data = try? JSONSerialization.data(withJSONObject: values) {
                reply.send(["task_id":"secret", "status":"ok", "result":String(decoding:data,as:UTF8.self)])
            } else { reply.send(["task_id":"secret", "status":"cancelled"]) }
        }
        active[id] = prompt
        let expires = Date().addingTimeInterval(900)
        prompt.timer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak prompt] _ in
            if !reply.isConnected || Date() >= expires { prompt?.cancel() }
        }
        if present { prompt.show() }
    }
}

struct HealthSnapshot: Decodable {
    struct Metrics: Decodable {
        var diskPath: String
        var diskTotalBytes: UInt64?
        var diskAvailableBytes: UInt64?
        var memoryTotalBytes: UInt64?
        var memoryAvailableBytes: UInt64?
        var memoryPressure: String
        var swapUsedBytes: UInt64?
    }
    struct Configuration: Decodable {
        var automatic: Bool
        var harvestProcesses: Bool
        var cleanWorktrees: Bool
        var intervalSeconds: UInt64
        var processMinAgeSeconds: UInt64
        var browserMinAgeSeconds: UInt64? = nil
        var observationSeconds: UInt64
        var worktreeMinAgeDays: UInt64
        var workspaceRoots: [String]
    }
    struct Worktree: Decodable {
        var path: String; var ageSeconds: UInt64?; var repository: String; var githubUrl: String?
    }
    struct LiveProcess: Decodable {
        var pid: UInt32; var parent: UInt32; var ageSeconds: UInt64
        var cpuPercent: Double; var residentBytes: UInt64; var executable: String
    }
    struct Item: Decodable {
        var name: String; var detail: String; var eligible: Bool
        var worktree: Worktree? = nil; var process: LiveProcess? = nil
        var selectionKey: String { worktree?.path ?? process.map { "\($0.pid):\($0.executable)" } ?? name }
    }
    struct Activity: Decodable { var at: Double; var category: String; var message: String }
    var observedAt: Double
    var lastCleanupAt: Double?
    var metrics: Metrics
    var config: Configuration
    var processes: [Item]
    var processInventory: [LiveProcess]?
    var worktrees: [Item]
    var harvestedProcesses: Int
    var removedWorktrees: Int
    var errors: [String]
    var activity: [Activity]?
    var running: Bool?
    var phase: String?
    static func decode(_ data: Data) throws -> HealthSnapshot {
        let decoder = JSONDecoder(); decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(Self.self, from: data)
    }
}

final class MachineHealthWindow: NSWindow {
    var copySelection: (() -> Bool)?
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let modifiers = event.modifierFlags.intersection([.command, .shift, .control, .option])
        guard modifiers == .command else { return super.performKeyEquivalent(with: event) }
        let key = event.charactersIgnoringModifiers?.lowercased()
        if key == "w" { performClose(nil); return true }
        if let text = firstResponder as? NSTextView {
            if key == "c" { text.copy(nil); return true }
            if key == "a" { text.selectAll(nil); return true }
            if key == "v" && text.isEditable { text.paste(nil); return true }
            if key == "x" && text.isEditable { text.cut(nil); return true }
        }
        if key == "c", copySelection?() == true { return true }
        return super.performKeyEquivalent(with: event)
    }
    override func sendEvent(_ event: NSEvent) {
        if event.type == .keyDown && performKeyEquivalent(with: event) { return }
        super.sendEvent(event)
    }
}

final class MachineHealth: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSWindowDelegate, NSSearchFieldDelegate {
    let window: MachineHealthWindow
    let machine = NSPopUpButton()
    let openRepository = NSButton(title: "Open GitHub", target: nil, action: nil)
    var selectedHost: String? { machine.indexOfSelectedItem > 0 ? machine.titleOfSelectedItem : nil }
    var generation = 0
    var readingHosts = false
    var nextHostRead = Date.distantPast
    let disk = NSTextField(labelWithString: "Checking disk space…")
    let memory = NSTextField(labelWithString: "Checking memory…")
    let memoryDetail = NSTextField(labelWithString: "")
    let diskBar = NSProgressIndicator()
    let automatic = NSButton(checkboxWithTitle: "Automatic cleanup", target: nil, action: nil)
    let processesEnabled = NSButton(checkboxWithTitle: "Harvest orphan processes", target: nil, action: nil)
    let worktreesEnabled = NSButton(checkboxWithTitle: "Clean unused worktrees", target: nil, action: nil)
    let policy = NSTextField(wrappingLabelWithString: "")
    let roots = NSTextField(wrappingLabelWithString: "")
    let footer = NSTextField(wrappingLabelWithString: "")
    let scan = NSButton(title: "Scan now", target: nil, action: nil)
    let clean = NSButton(title: "Clean eligible items", target: nil, action: nil)
    let addFolder = NSButton(title: "Add workspace…", target: nil, action: nil)
    let kind = NSSegmentedControl(labels: ["Processes", "Worktrees", "Activity"], trackingMode: .selectOne, target: nil, action: nil)
    let logSearch = NSSearchField()
    let selectedEvent = NSTextField(wrappingLabelWithString: "Select an entry to read its full message.")
    let currentPhase = NSTextField(labelWithString: "")
    let table = NSTableView()
    var snapshot: HealthSnapshot?
    var busy = false
    var readingStatus = false
    var timer: Timer?
    var runner: (([String], @escaping (Result<Data, Error>) -> Void) -> Void)?
    var cli: String?
    let present: Bool
    var items: [HealthSnapshot.Item] = []
    func rebuildItems() {
        let all: [HealthSnapshot.Item]
        if kind.selectedSegment == 2 {
            all = (snapshot?.activity ?? []).reversed().map { event in
                let stamp = Date(timeIntervalSince1970: event.at).formatted(date: .abbreviated, time: .standard)
                return .init(name: "\(stamp) · \(event.category)", detail: event.message, eligible: false)
            }
        } else if kind.selectedSegment == 1 { all = snapshot?.worktrees ?? [] }
        else if let inventory = snapshot?.processInventory {
            all = inventory.map { process in
                .init(name: "\((process.executable as NSString).lastPathComponent) · PID \(process.pid)",
                      detail: "Parent PID \(process.parent) · \(process.executable)", eligible: false, process: process)
            }
        } else { all = snapshot?.processes ?? [] }
        let query = logSearch.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let selectedKey = items.indices.contains(table.selectedRow) ? items[table.selectedRow].selectionKey : nil
        items = query.isEmpty ? all : all.filter { ($0.name + " " + $0.detail + " " + ($0.worktree?.repository ?? "")).localizedCaseInsensitiveContains(query) }
        table.tableColumn(withIdentifier: .init("name"))?.title = kind.selectedSegment == 2 ? "Time · Category" : kind.selectedSegment == 0 ? "Process · PID" : "Checkout"
        table.tableColumn(withIdentifier: .init("state"))?.title = kind.selectedSegment == 2 ? "Activity" : kind.selectedSegment == 0 ? "Process details" : "Status"
        table.tableColumn(withIdentifier: .init("age"))?.isHidden = kind.selectedSegment == 2
        table.tableColumn(withIdentifier: .init("repository"))?.isHidden = kind.selectedSegment != 1
        for id in ["memory", "cpu"] { table.tableColumn(withIdentifier: .init(id))?.isHidden = kind.selectedSegment != 0 }
        clean.title = kind.selectedSegment == 1 ? "Remove selected worktree" : "Clean eligible items"
        openRepository.isHidden = kind.selectedSegment != 1
        table.deselectAll(nil); table.reloadData()
        if let selectedKey, let row = items.firstIndex(where: { $0.selectionKey == selectedKey }) { table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false) }
        if items.isEmpty {
            selectedEvent.stringValue = !query.isEmpty ? "No entries match this filter." : kind.selectedSegment == 0 ? "Process list unavailable. Check the connection or update this machine’s health worker." : "No entries yet."
        }
        setBusy(busy)
    }
    init(present: Bool = true, cli: String? = nil) {
        self.present = present; self.cli = cli
        window = MachineHealthWindow(contentRect: NSRect(x: 0, y: 0, width: 920, height: 680), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = "hey-boss · Machine Health"; window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 820, height: 640)
        super.init(); window.delegate = self
        window.copySelection = { [weak self] in self?.copySelected() ?? false }
        let content = OverviewCanvas(frame: window.contentView!.bounds); window.contentView = content
        let title = NSTextField(labelWithString: "Machine Health"); title.font = .systemFont(ofSize: 24, weight: .semibold)
        machine.addItem(withTitle: "This Mac"); machine.target = self; machine.action = #selector(switchMachine)
        machine.setAccessibilityLabel("Machine")
        openRepository.target = self; openRepository.action = #selector(openGitHub); openRepository.isHidden = true
        let subtitle = NSTextField(labelWithString: "Storage, memory, and maintenance on this Mac and SSH clients")
        subtitle.textColor = .secondaryLabelColor
        let diskTitle = NSTextField(labelWithString: "Disk space"); let memoryTitle = NSTextField(labelWithString: "Memory")
        for label in [diskTitle, memoryTitle] { label.font = .systemFont(ofSize: 15, weight: .semibold) }
        for label in [disk, memory] { label.font = .monospacedDigitSystemFont(ofSize: 13, weight: .regular) }
        memoryDetail.font = .monospacedDigitSystemFont(ofSize: 12, weight: .regular); memoryDetail.textColor = .secondaryLabelColor
        diskBar.isIndeterminate = false; diskBar.minValue = 0; diskBar.maxValue = 100; diskBar.style = .bar
        let cleanupTitle = NSTextField(labelWithString: "Keep the machine tidy"); cleanupTitle.font = .systemFont(ofSize: 16, weight: .semibold)
        automatic.target = self; automatic.action = #selector(toggleAutomatic)
        processesEnabled.target = self; processesEnabled.action = #selector(toggleProcesses)
        worktreesEnabled.target = self; worktreesEnabled.action = #selector(toggleWorktrees)
        scan.target = self; scan.action = #selector(scanNow); clean.target = self; clean.action = #selector(cleanNow)
        addFolder.target = self; addFolder.action = #selector(addWorkspace)
        kind.selectedSegment = 0; kind.target = self; kind.action = #selector(switchKind)
        logSearch.placeholderString = "Filter activity, processes, or worktrees…"; logSearch.delegate = self
        selectedEvent.font = .systemFont(ofSize: 12); selectedEvent.textColor = .secondaryLabelColor
        selectedEvent.isSelectable = true
        selectedEvent.maximumNumberOfLines = 3; selectedEvent.lineBreakMode = .byTruncatingTail
        for label in [disk, memory, memoryDetail, policy, roots, footer, selectedEvent, currentPhase] { label.isSelectable = true }
        currentPhase.font = .systemFont(ofSize: 12, weight: .medium); currentPhase.textColor = .secondaryLabelColor
        for label in [policy, roots, footer] { label.font = .systemFont(ofSize: 12); label.textColor = .secondaryLabelColor }
        roots.maximumNumberOfLines = 2; roots.lineBreakMode = .byTruncatingTail
        let name = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("name")); name.title = "Item"; name.width = 360
        let state = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("state")); state.title = "Status"; state.width = 470
        name.width = 260
        let age = NSTableColumn(identifier: .init("age")); age.title = "Age"; age.width = 65; age.isHidden = true
        let repository = NSTableColumn(identifier: .init("repository")); repository.title = "GitHub repository"; repository.width = 180; repository.isHidden = true
        let processMemory = NSTableColumn(identifier: .init("memory")); processMemory.title = "Memory (RSS)"; processMemory.width = 100
        let cpu = NSTableColumn(identifier: .init("cpu")); cpu.title = "CPU %"; cpu.width = 65
        table.addTableColumn(name); table.addTableColumn(age); table.addTableColumn(processMemory); table.addTableColumn(cpu); table.addTableColumn(repository); table.addTableColumn(state); table.dataSource = self; table.delegate = self
        table.style = .inset; table.rowHeight = 36; table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        let scroll = NSScrollView(); scroll.documentView = table; scroll.hasVerticalScroller = true; scroll.autohidesScrollers = true
        let views: [NSView] = [title, subtitle, machine, openRepository, diskTitle, disk, diskBar, memoryTitle, memory, memoryDetail, cleanupTitle, automatic, processesEnabled, worktreesEnabled, policy, roots, addFolder, kind, scan, clean, logSearch, currentPhase, scroll, selectedEvent, footer]
        for view in views { view.translatesAutoresizingMaskIntoConstraints = false; content.addSubview(view) }
        NSLayoutConstraint.activate([
            machine.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), machine.centerYAnchor.constraint(equalTo: title.centerYAnchor), machine.widthAnchor.constraint(equalToConstant: 245),
            openRepository.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), openRepository.topAnchor.constraint(equalTo: selectedEvent.topAnchor), openRepository.widthAnchor.constraint(equalToConstant: 120),
            title.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 24), title.topAnchor.constraint(equalTo: content.topAnchor, constant: 24),
            subtitle.leadingAnchor.constraint(equalTo: title.leadingAnchor), subtitle.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 5),
            diskTitle.leadingAnchor.constraint(equalTo: title.leadingAnchor), diskTitle.topAnchor.constraint(equalTo: subtitle.bottomAnchor, constant: 26),
            disk.leadingAnchor.constraint(equalTo: title.leadingAnchor), disk.topAnchor.constraint(equalTo: diskTitle.bottomAnchor, constant: 7),
            diskBar.leadingAnchor.constraint(equalTo: title.leadingAnchor), diskBar.topAnchor.constraint(equalTo: disk.bottomAnchor, constant: 10), diskBar.widthAnchor.constraint(equalTo: content.widthAnchor, multiplier: 0.43),
            memoryTitle.leadingAnchor.constraint(equalTo: content.centerXAnchor, constant: 16), memoryTitle.topAnchor.constraint(equalTo: diskTitle.topAnchor),
            memory.leadingAnchor.constraint(equalTo: memoryTitle.leadingAnchor), memory.topAnchor.constraint(equalTo: memoryTitle.bottomAnchor, constant: 7), memory.trailingAnchor.constraint(lessThanOrEqualTo: content.trailingAnchor, constant: -24),
            memoryDetail.leadingAnchor.constraint(equalTo: memory.leadingAnchor), memoryDetail.topAnchor.constraint(equalTo: memory.bottomAnchor, constant: 7), memoryDetail.trailingAnchor.constraint(lessThanOrEqualTo: content.trailingAnchor, constant: -24),
            cleanupTitle.leadingAnchor.constraint(equalTo: title.leadingAnchor), cleanupTitle.topAnchor.constraint(equalTo: diskBar.bottomAnchor, constant: 27),
            automatic.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), automatic.centerYAnchor.constraint(equalTo: cleanupTitle.centerYAnchor),
            processesEnabled.leadingAnchor.constraint(equalTo: title.leadingAnchor), processesEnabled.topAnchor.constraint(equalTo: cleanupTitle.bottomAnchor, constant: 12),
            worktreesEnabled.leadingAnchor.constraint(equalTo: processesEnabled.trailingAnchor, constant: 24), worktreesEnabled.centerYAnchor.constraint(equalTo: processesEnabled.centerYAnchor),
            policy.leadingAnchor.constraint(equalTo: title.leadingAnchor), policy.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), policy.topAnchor.constraint(equalTo: processesEnabled.bottomAnchor, constant: 10),
            roots.leadingAnchor.constraint(equalTo: title.leadingAnchor), roots.trailingAnchor.constraint(equalTo: addFolder.leadingAnchor, constant: -16), roots.topAnchor.constraint(equalTo: policy.bottomAnchor, constant: 12),
            addFolder.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), addFolder.topAnchor.constraint(equalTo: roots.topAnchor),
            kind.leadingAnchor.constraint(equalTo: title.leadingAnchor), kind.topAnchor.constraint(equalTo: roots.bottomAnchor, constant: 22),
            clean.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), clean.centerYAnchor.constraint(equalTo: kind.centerYAnchor),
            scan.trailingAnchor.constraint(equalTo: clean.leadingAnchor, constant: -8), scan.centerYAnchor.constraint(equalTo: kind.centerYAnchor),
            logSearch.leadingAnchor.constraint(equalTo: title.leadingAnchor), logSearch.topAnchor.constraint(equalTo: kind.bottomAnchor, constant: 12), logSearch.widthAnchor.constraint(equalTo: content.widthAnchor, multiplier: 0.48),
            currentPhase.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), currentPhase.centerYAnchor.constraint(equalTo: logSearch.centerYAnchor),
            scroll.leadingAnchor.constraint(equalTo: title.leadingAnchor), scroll.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), scroll.topAnchor.constraint(equalTo: logSearch.bottomAnchor, constant: 10), scroll.bottomAnchor.constraint(equalTo: selectedEvent.topAnchor, constant: -8),
            selectedEvent.leadingAnchor.constraint(equalTo: title.leadingAnchor), selectedEvent.trailingAnchor.constraint(equalTo: openRepository.leadingAnchor, constant: -12), selectedEvent.bottomAnchor.constraint(equalTo: footer.topAnchor, constant: -10), selectedEvent.heightAnchor.constraint(equalToConstant: 46),
            footer.leadingAnchor.constraint(equalTo: title.leadingAnchor), footer.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), footer.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -20),
        ])
    }
    deinit { timer?.invalidate() }
    static func bytes(_ value: UInt64?) -> String { value.map { String(format: "%.1f GB", Double($0) / 1_000_000_000) } ?? "Unavailable" }
    func render(_ value: HealthSnapshot) {
        snapshot = value
        // Keep a selected table cell intact while copying; metrics and counts still refresh.
        let selectedEditor = window.firstResponder as? NSTextView
        let selectingCell = (selectedEditor?.selectedRange().length ?? 0) > 0 && (selectedEditor?.delegate as? NSView)?.isDescendant(of: table) == true
        let m = value.metrics
        disk.stringValue = "\(Self.bytes(m.diskAvailableBytes)) available of \(Self.bytes(m.diskTotalBytes))"
        diskBar.doubleValue = m.diskTotalBytes.flatMap { total in total > 0 ? m.diskAvailableBytes.map { 100 * (1 - Double($0) / Double(total)) } : nil } ?? 0
        memory.stringValue = "\(m.memoryPressure) pressure · \(Self.bytes(m.memoryTotalBytes)) RAM"
        memory.textColor = m.memoryPressure == "Critical" ? .systemRed : m.memoryPressure == "Warning" ? .systemOrange : .labelColor
        memoryDetail.stringValue = "\(Self.bytes(m.memoryAvailableBytes)) available (est.) · \(Self.bytes(m.swapUsedBytes)) swap"
        memory.toolTip = "Estimated available: \(Self.bytes(m.memoryAvailableBytes)) · Swap used: \(Self.bytes(m.swapUsedBytes))"
        automatic.state = value.config.automatic ? .on : .off
        processesEnabled.state = value.config.harvestProcesses ? .on : .off
        worktreesEnabled.state = value.config.cleanWorktrees ? .on : .off
        policy.stringValue = "Checks every \(value.config.intervalSeconds / 60) minutes. Test browsers: \((value.config.browserMinAgeSeconds ?? value.config.processMinAgeSeconds) / 60)+ minutes old; other test processes: \(value.config.processMinAgeSeconds / 60)+ minutes. Cleanup requires repeated quiet checks and no clients. Worktrees: unused, clean, merged, \(value.config.worktreeMinAgeDays)+ days. Codex stays running."
        roots.stringValue = "Workspaces: " + (value.config.workspaceRoots.isEmpty ? "None configured" : value.config.workspaceRoots.joined(separator: " · "))
        roots.toolTip = value.config.workspaceRoots.joined(separator: "\n")
        kind.setLabel("Processes (\(value.processInventory?.count ?? value.processes.count))", forSegment: 0); kind.setLabel("Worktrees (\(value.worktrees.count))", forSegment: 1)
        kind.setLabel("Activity (\(value.activity?.count ?? 0))", forSegment: 2)
        currentPhase.stringValue = value.phase?.isEmpty == false ? value.phase! : "Waiting for the next check"
        currentPhase.textColor = value.running == true ? .systemBlue : .secondaryLabelColor
        let date = value.observedAt > 0 ? Date(timeIntervalSince1970: value.observedAt).formatted(date: .abbreviated, time: .shortened) : "Not scanned yet"
        footer.stringValue = value.errors.isEmpty ? "\(date) · Last scan: \(value.processes.count) cleanup groups; stopped \(value.harvestedProcesses) processes; removed \(value.removedWorktrees) worktrees." : value.errors.joined(separator: "\n")
        footer.textColor = value.errors.isEmpty ? .secondaryLabelColor : .systemOrange
        if !selectingCell { rebuildItems() }; setBusy(busy)
    }
    func show() {
        if present { NSApplication.shared.setActivationPolicy(.regular) }
        window.center(); window.makeKeyAndOrderFront(nil); NSApplication.shared.activate(ignoringOtherApps: true)
        nextHostRead = .distantPast; refresh()
        timer?.invalidate(); timer = Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in self?.refresh() }
    }
    func windowWillClose(_ notification: Notification) { timer?.invalidate(); timer = nil; if present { NSApplication.shared.setActivationPolicy(.accessory) } }
    func numberOfRows(in tableView: NSTableView) -> Int { items.count }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard items.indices.contains(row) else { return nil }
        let item = items[row]; let column = tableColumn?.identifier.rawValue
        let value: String
        switch column {
        case "name": value = item.name
        case "age": value = Self.age(item.process?.ageSeconds ?? item.worktree?.ageSeconds)
        case "memory": value = Self.bytes(item.process?.residentBytes)
        case "cpu": value = item.process.map { String(format: "%.1f", $0.cpuPercent) } ?? "—"
        case "repository": value = item.worktree?.repository ?? "Unknown"
        default: value = item.detail
        }
        let label = NSTextField(labelWithString: value)
        label.isSelectable = true
        label.font = .systemFont(ofSize: 12); label.lineBreakMode = .byTruncatingMiddle
        label.toolTip = column == "memory" ? "Resident RAM (RSS), excluding swapped memory. Sorted largest first." : column == "cpu" ? "CPU usage reported by ps; averaging differs by operating system." : column == "age" && item.worktree != nil ? "Checkout age (Git file creation time, or modification time when unavailable). Automatic cleanup also checks the latest activity." : value
        label.textColor = column != "state" ? .labelColor : item.eligible ? .systemGreen : .secondaryLabelColor
        return label
    }
    static func age(_ seconds: UInt64?) -> String {
        guard let seconds else { return "Unknown" }
        if seconds < 3600 { return "\(seconds / 60)m" }
        if seconds < 86400 { return "\(seconds / 3600)h" }
        return "\(seconds / 86400)d"
    }
    @objc func switchKind() {
        table.deselectAll(nil)
        selectedEvent.stringValue = kind.selectedSegment == 1 ? "Select a checkout to remove. Its branch is retained; active work and local files are protected." : "Select an entry to read its full message."
        rebuildItems()
    }
    func updateHosts(_ hosts: [String]) {
        let selected = selectedHost
        machine.removeAllItems(); machine.addItem(withTitle: "This Mac")
        for host in hosts { machine.addItem(withTitle: host) }
        if let selected {
            if !hosts.contains(selected) { machine.addItem(withTitle: selected) }
            machine.selectItem(withTitle: selected)
        }
    }
    @objc func switchMachine() {
        generation += 1; snapshot = nil; items = []; readingStatus = false; busy = false
        table.deselectAll(nil); table.reloadData()
        disk.stringValue = "Connecting…"; diskBar.doubleValue = 0; memory.stringValue = "Connecting…"; memoryDetail.stringValue = ""
        roots.stringValue = ""; policy.stringValue = ""; footer.stringValue = ""
        selectedEvent.stringValue = ""; currentPhase.stringValue = "Loading selected machine…"
        window.title = "hey-boss · Health · " + (selectedHost ?? "This Mac")
        setBusy(false); refresh()
    }
    @objc func openGitHub() {
        guard kind.selectedSegment == 1, items.indices.contains(table.selectedRow),
              let raw = items[table.selectedRow].worktree?.githubUrl,
              let url = URL(string: raw), url.scheme == "https", url.host == "github.com" else { return }
        NSWorkspace.shared.open(url)
    }
    func controlTextDidChange(_ obj: Notification) { rebuildItems() }
    func tableViewSelectionDidChange(_ notification: Notification) {
        if items.indices.contains(table.selectedRow) {
            let item = items[table.selectedRow]
            if let editor = selectedEvent.currentEditor() as? NSTextView, editor.selectedRange().length > 0 { setBusy(busy); return }
            selectedEvent.stringValue = item.name + "\n" + (item.process.map { "\(Self.bytes($0.residentBytes)) RSS · \(String(format: "%.1f", $0.cpuPercent))% CPU · \(Self.age($0.ageSeconds)) old · " } ?? "") + (item.worktree.map { "\($0.repository) · \(Self.age($0.ageSeconds)) old · " } ?? "") + item.detail
            selectedEvent.toolTip = selectedEvent.stringValue
        }
        setBusy(busy)
    }
    @discardableResult func copySelected(to pasteboard: NSPasteboard = .general) -> Bool {
        guard items.indices.contains(table.selectedRow) else { return false }
        let item = items[table.selectedRow]
        let columns = item.worktree.map { [item.name, Self.age($0.ageSeconds), $0.repository, item.detail] } ?? item.process.map { [item.name, Self.age($0.ageSeconds), Self.bytes($0.residentBytes), String(format: "%.1f%% CPU", $0.cpuPercent), item.detail] } ?? [item.name, item.detail]
        pasteboard.clearContents()
        return pasteboard.setString(columns.joined(separator: "\t"), forType: .string)
    }
    @objc func scanNow() { request(["scan", "--json"], snapshotResult: true) }
    @objc func cleanNow() {
        if kind.selectedSegment == 1 {
            guard items.indices.contains(table.selectedRow), let path = items[table.selectedRow].worktree?.path else { return }
            request(["remove-worktree", path, "--json"], snapshotResult: true)
        } else { request(["clean", "--json"], snapshotResult: true) }
    }
    @objc func toggleAutomatic() { request([automatic.state == .on ? "enable" : "disable"], snapshotResult: false) }
    @objc func toggleProcesses() { request(["configure", "--processes", processesEnabled.state == .on ? "true" : "false"], snapshotResult: false) }
    @objc func toggleWorktrees() { request(["configure", "--worktrees", worktreesEnabled.state == .on ? "true" : "false"], snapshotResult: false) }
    @objc func addWorkspace() {
        if selectedHost != nil {
            let alert = NSAlert(); alert.messageText = "Add workspace on " + selectedHost!; alert.informativeText = "Enter an absolute directory path on this machine."
            let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 420, height: 24)); field.placeholderString = "/home/user/Workspace"
            alert.accessoryView = field; alert.addButton(withTitle: "Add workspace"); alert.addButton(withTitle: "Cancel")
            alert.beginSheetModal(for: window) { [weak self] result in
                if result == .alertFirstButtonReturn { self?.request(["add-root", field.stringValue], snapshotResult: false) }
            }
            return
        }
        let picker = NSOpenPanel(); picker.canChooseDirectories = true; picker.canChooseFiles = false; picker.prompt = "Add workspace"
        picker.beginSheetModal(for: window) { [weak self] response in
            if response == .OK, let path = picker.url?.path { self?.request(["add-root", path], snapshotResult: false) }
        }
    }
    func refresh() {
        if present && nextHostRead <= Date() && !readingHosts { nextHostRead = Date().addingTimeInterval(30); request(["hosts", "--json"], snapshotResult: false) }
        request(["status", "--json"], snapshotResult: true)
    }
    func setBusy(_ value: Bool) {
        busy = value
        for button in [scan, clean, automatic, processesEnabled, worktreesEnabled, addFolder] { button.isEnabled = !value && snapshot != nil && snapshot?.running != true }
        machine.isEnabled = !value
        let worktree = items.indices.contains(table.selectedRow) ? items[table.selectedRow].worktree : nil
        if kind.selectedSegment == 1 { clean.isEnabled = clean.isEnabled && worktree != nil }
        openRepository.isEnabled = worktree?.githubUrl != nil
    }
    func request(_ args: [String], snapshotResult: Bool) {
        let hostsRequest = args.first == "hosts"
        let statusRequest = args.first == "status"
        let requestGeneration = generation
        let routedArgs = !hostsRequest && selectedHost != nil ? ["--host", selectedHost!] + args : args
        if hostsRequest { guard !readingHosts else { return }; readingHosts = true }
        else if statusRequest { guard !readingStatus else { return }; readingStatus = true }
        else { guard !busy else { return }; setBusy(true); currentPhase.stringValue = "Starting health check…" }
        let completion: (Result<Data, Error>) -> Void = { [weak self] result in
            onMain {
                guard let self else { return }
                if hostsRequest { self.readingHosts = false }
                else {
                    guard self.generation == requestGeneration else { return }
                    if statusRequest { self.readingStatus = false } else { self.setBusy(false) }
                }
                do {
                    let data = try result.get()
                    if hostsRequest { self.updateHosts(try JSONDecoder().decode([String].self, from: data)) }
                    else if snapshotResult { self.render(try HealthSnapshot.decode(data)) } else { self.refresh() }
                } catch {
                    if let snapshot = self.snapshot { self.render(snapshot) }
                    self.footer.stringValue = "Health check failed on \(self.selectedHost ?? "This Mac"): \(error.localizedDescription)"; self.footer.textColor = .systemOrange
                }
            }
        }
        if let runner { runner(routedArgs, completion); return }
        let candidates = [cli, ProcessInfo.processInfo.environment["HEY_BOSS_CLI_PATH"], "/opt/homebrew/bin/hey-boss", "/usr/local/bin/hey-boss"].compactMap { $0 }
        guard let path = candidates.first(where: { FileManager.default.isExecutableFile(atPath: $0) }) else {
            completion(.failure(StorageError(description: "Install the updated hey-boss CLI."))); return
        }
        DispatchQueue.global(qos: .utility).async {
            let process = Process(); process.executableURL = URL(fileURLWithPath: path); process.arguments = ["health"] + routedArgs
            let pipe = Pipe(); process.standardOutput = pipe; process.standardError = pipe
            defer { try? pipe.fileHandleForReading.close() }
            do {
                try process.run()
                // Status is cheap; a full worktree inspection may take several minutes.
                DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 300) {
                    if process.isRunning {
                        process.terminate()
                        DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 3) { if process.isRunning { Darwin.kill(process.processIdentifier, SIGKILL) } }
                    }
                }
                let data = try readScannerOutput(pipe.fileHandleForReading); process.waitUntilExit()
                guard process.terminationStatus == 0 else { throw StorageError(description: String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? "Maintenance is busy or inspection failed.") }
                completion(.success(data))
            } catch {
                if process.isRunning {
                    process.terminate()
                    DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 3) { if process.isRunning { Darwin.kill(process.processIdentifier, SIGKILL) } }
                    process.waitUntilExit()
                }
                completion(.failure(error))
            }
        }
    }
}

final class AgentOverviewWindow: NSWindow {
    var eventAgeSamples: [Double] = []
    override func sendEvent(_ event: NSEvent) {
        if event.type == .leftMouseDown {
            eventAgeSamples.append(max(0, (ProcessInfo.processInfo.systemUptime - event.timestamp) * 1000))
            if eventAgeSamples.count > 60 { eventAgeSamples.removeFirst() }
        }
        super.sendEvent(event)
    }
    var onFind: (() -> Void)?
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.modifierFlags.intersection([.command, .shift, .control, .option]) == .command,
           event.charactersIgnoringModifiers?.lowercased() == "f", let onFind {
            onFind()
            return true
        }
        return super.performKeyEquivalent(with: event)
    }
}

enum MenuActivity: String {
    case active, idle, confirming, away, locked, offline, unknown
    static func state(_ routing: [String: Any]) -> Self {
        guard let state = routing["macState"] as? String else { return .unknown }
        if state == "active" {
            guard let idle = routing["macIdleSeconds"] as? Double, idle.isFinite, idle >= 0 else { return .unknown }
            return idle >= 30 ? .idle : .active
        }
        return Self(rawValue: state) ?? .unknown
    }
    var label: String {
        switch self {
        case .active: return "Mac active"
        case .idle: return "Mac idle"
        case .confirming: return "Confirming away"
        case .away: return "Mac away"
        case .locked: return "Mac locked or asleep"
        case .offline: return "Mac disconnected"
        case .unknown: return "Activity status unavailable"
        }
    }
}
// A status affordance beside the existing SF Symbol, not a second icon.
final class ActivityBadge: NSView {
    var state: MenuActivity = .unknown { didSet { if state != oldValue { needsDisplay = true } } }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) {
        let dot = NSBezierPath(ovalIn: bounds.insetBy(dx: 1, dy: 1))
        NSColor.windowBackgroundColor.setStroke(); dot.lineWidth = 2; dot.stroke()
        switch state {
        case .active: NSColor.systemGreen.setFill(); dot.fill()
        case .confirming: NSColor.systemOrange.setFill(); dot.fill()
        case .away: NSColor.systemIndigo.setFill(); dot.fill()
        case .locked, .offline: NSColor.secondaryLabelColor.setFill(); dot.fill()
        case .idle, .unknown: NSColor.secondaryLabelColor.setStroke(); dot.lineWidth = 1; dot.stroke()
        }
    }
}
final class SteeringTextView: NSTextView {
    var send: (() -> Void)?
    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        if string.isEmpty {
            let hint = NSAttributedString(string: "Give this active turn an instruction…", attributes: [.font: font ?? NSFont.systemFont(ofSize: 12), .foregroundColor: NSColor.placeholderTextColor])
            hint.draw(at: NSPoint(x: textContainerInset.width + 5, y: textContainerInset.height))
        }
    }
    override func keyDown(with event: NSEvent) {
        if event.modifierFlags.contains(.command), event.keyCode == 36 { send?(); return }
        super.keyDown(with: event)
    }
}

final class AgentControlPanel: NSStackView {
    let status = NSTextField(wrappingLabelWithString: "Connect to manage the saved goal or steer this session.")
    let objective = NSTextField(wrappingLabelWithString: "")
    let goalButton = NSButton(title: "Re-enable goal", target: nil, action: nil)
    let refreshButton = NSButton(title: "Connect controls", target: nil, action: nil)
    let sendButton = NSButton(title: "Send instruction", target: nil, action: nil)
    let editor = SteeringTextView()
    var perform: ((String, [String: Any], @escaping ([String: Any]) -> Void) -> Void)?
    var goalStatus: String?
    var turnId: String?
    var canSteer = false
    var busy = false
    init() {
        super.init(frame: .zero)
        orientation = .vertical; alignment = .leading; spacing = 6
        status.font = .systemFont(ofSize: 11); status.textColor = .secondaryLabelColor; status.maximumNumberOfLines = 3
        objective.font = .systemFont(ofSize: 11); objective.maximumNumberOfLines = 2; objective.isSelectable = true
        goalButton.target = self; goalButton.action = #selector(toggleGoal); goalButton.isEnabled = false
        refreshButton.target = self; refreshButton.action = #selector(connect)
        sendButton.target = self; sendButton.action = #selector(sendInstruction); sendButton.isEnabled = false
        let actions = NSStackView(views: [goalButton, refreshButton]); actions.orientation = .horizontal; actions.spacing = 8
        editor.isRichText = false; editor.font = .systemFont(ofSize: 12); editor.textContainerInset = NSSize(width: 8, height: 7)
        editor.isAutomaticQuoteSubstitutionEnabled = false; editor.isAutomaticDashSubstitutionEnabled = false
        editor.setAccessibilityLabel("Instruction for this Codex session")
        editor.send = { [weak self] in self?.sendInstruction() }
        let scroll = NSScrollView(); scroll.hasVerticalScroller = true; scroll.borderType = .bezelBorder; scroll.documentView = editor
        editor.isVerticallyResizable = true; editor.isHorizontallyResizable = false; editor.autoresizingMask = [.width]
        editor.textContainer?.widthTracksTextView = true
        for view in [status, objective, actions, scroll, sendButton] { addArrangedSubview(view) }
        scroll.heightAnchor.constraint(equalToConstant: 60).isActive = true
        for view in [status, objective, scroll] { view.widthAnchor.constraint(equalTo: widthAnchor).isActive = true }
        sendButton.toolTip = "Send to the verified active turn (⌘Return). Your draft clears after Codex acknowledges it."
    }
    required init?(coder: NSCoder) { nil }
    @objc func connect() { request("inspect") }
    @objc func toggleGoal() { request(goalStatus == "active" ? "disable-goal" : "enable-goal") }
    @objc func sendInstruction() {
        guard canSteer, let turnId, !editor.string.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        request("steer", input: ["text": editor.string, "expectedTurnId": turnId])
    }
    func request(_ action: String, input: [String: Any] = [:]) {
        guard !busy, let perform else { return }
        busy = true; goalButton.isEnabled = false; sendButton.isEnabled = false; refreshButton.isEnabled = false
        editor.isEditable = action != "steer"
        status.stringValue = action == "inspect" ? "Checking session connection…" : "Waiting for Codex acknowledgement…"
        status.textColor = .secondaryLabelColor
        perform(action, input) { [weak self] response in
            guard let self else { return }
            self.busy = false; self.refreshButton.isEnabled = true; self.editor.isEditable = true
            self.refreshButton.title = "Refresh controls"
            if response["ok"] as? Bool == true {
                let goal = response["goal"] as? [String: Any]
                self.goalStatus = goal?["status"] as? String
                self.turnId = response["turnId"] as? String
                self.canSteer = response["canSteer"] as? Bool == true
                self.goalButton.title = self.goalStatus == "active" ? "Pause goal" : "Re-enable goal"
                self.goalButton.isEnabled = goal != nil
                self.sendButton.isEnabled = self.canSteer
                self.objective.stringValue = goal?["objective"] as? String ?? "No saved goal in this session."
                self.objective.toolTip = self.objective.stringValue
                self.status.stringValue = action == "steer" ? "Instruction accepted by Codex." : "Goal: \(self.goalStatus ?? "none") · \(self.canSteer ? "Ready to steer · ⌘Return to send" : "No steerable active turn")"
                if action == "steer" { self.editor.string = "" }
            } else {
                self.status.stringValue = response["error"] as? String ?? "Unable to control this session."
                self.status.textColor = .systemOrange
                self.canSteer = false; self.turnId = nil
                self.goalButton.isEnabled = false; self.sendButton.isEnabled = false
            }
        }
    }
}

/// Opens the normal browser, reusing or starting the local issue service.
final class IssuesLauncher {
    let url = URL(string: "http://127.0.0.1:4781/")!
    var destination = URL(string: "http://127.0.0.1:4781/")!
    var launching = false
    var child: Process?
    var openURL: (URL) -> Bool = { NSWorkspace.shared.open($0) }
    var report: (String) -> Void = { message in
        let alert = NSAlert(); alert.messageText = "Could not open Issues"; alert.informativeText = message
        alert.addButton(withTitle: "OK"); NSApp.activate(ignoringOtherApps: true); alert.runModal()
    }
    var probe: (@escaping (Bool) -> Void) -> Void = { done in
        var request = URLRequest(url: URL(string: "http://127.0.0.1:4781/api/bootstrap")!)
        request.timeoutInterval = 1
        URLSession.shared.dataTask(with: request) { data, response, _ in
            let value = data.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
            let valid = (response as? HTTPURLResponse)?.statusCode == 200 && value?["ok"] as? Bool == true && value?["projects"] is [[String: Any]] && value?["csrf"] is String
            onMain { done(valid) }
        }.resume()
    }
    var start: ((String) throws -> Void)?
    func open(cli: String?, inbox: Bool = false) {
        destination = inbox ? URL(string: "http://127.0.0.1:4781/#view=inbox")! : url
        guard !launching else { return }
        launching = true
        probe { [weak self] ready in
            guard let self else { return }
            if ready { self.finish(); return }
            guard let cli, FileManager.default.isExecutableFile(atPath: cli) else { self.fail("Install the updated hey-boss CLI, then try again."); return }
            do {
                if let start = self.start { try start(cli) }
                else {
                    let process = Process()
                    process.executableURL = URL(fileURLWithPath: cli)
                    process.arguments = ["issue", "web", "--port", "4781", "--json"]
                    process.currentDirectoryURL = FileManager.default.homeDirectoryForCurrentUser
                    process.standardOutput = FileHandle.nullDevice
                    process.standardError = FileHandle.standardError
                    try process.run()
                    self.child = process
                }
                self.waitForServer(remaining: 30)
            } catch { self.fail(String(describing: error)) }
        }
    }
    func waitForServer(remaining: Int) {
        probe { [weak self] ready in
            guard let self else { return }
            if ready { self.finish() }
            else if remaining == 0 || self.child.map({ !$0.isRunning }) == true {
                self.fail("The issue service could not start on port 4781. Check that this port is available and your hey-boss CLI is up to date.")
            } else { DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { self.waitForServer(remaining: remaining - 1) } }
        }
    }
    func finish() { launching = false; if !openURL(destination) { report("Open \(destination.absoluteString) in your browser.") } }
    func fail(_ message: String) { launching = false; report(message) }
}

final class AgentsOverview: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate, NSMenuItemValidation, NSWindowDelegate {
    var openInbox: (() -> Void)?
    @objc func showInbox() { if let openInbox { openInbox() } else { issuesLauncher.open(cli:cli,inbox:true) } }
    var openIssues: (() -> Void)?
    lazy var issuesLauncher = IssuesLauncher()
    @objc func showIssues() { if let openIssues { openIssues() } else { issuesLauncher.open(cli: cli) } }
    let inboxMenuItem = NSMenuItem(title: "Inbox…", action: nil, keyEquivalent: "")
    let issuesMenuItem = NSMenuItem(title: "Issues…", action: nil, keyEquivalent: "")
    let statusMenu = NSMenu()
    let activityMenuItem = NSMenuItem(title: "Activity status unavailable", action: nil, keyEquivalent: "")
    let activityBadge = ActivityBadge(frame: NSRect(x: 15, y: 2, width: 7, height: 7))
    var inboxCount = 0
    var activityRouting: [String: Any] = [:]
    func updateInboxCount(_ count: Int) {
        inboxCount = max(0, count)
        inboxMenuItem.title = count == 0 ? "Inbox…" : "Inbox · \(count) unread…"
        updateActivity(activityRouting)
    }
    func updateActivity(_ routing: [String: Any]) {
        activityRouting = routing
        let state = MenuActivity.state(routing)
        activityBadge.state = state
        let phone = routing["notifyPhone"] as? Bool
        let delivery = phone == true ? "Phone pushes enabled" : phone == false ? "Phone pushes paused" : "Phone routing unknown"
        activityMenuItem.title = "\(state.label) · \(delivery.lowercased())"
        var parts = ["Hey Boss", state.label, delivery]
        if let idle = routing["macIdleSeconds"] as? Double, idle.isFinite, idle >= 0 {
            parts.append(idle < 60 ? "Last input \(Int(idle))s ago" : "Last input \(Int(min(idle, 604800) / 60))m ago")
        }
        if inboxCount > 0 { parts.append("\(inboxCount) unread") }
        let description = parts.joined(separator: " · ")
        statusItem?.button?.toolTip = description
        statusItem?.button?.setAccessibilityLabel(description)
    }
    let window: AgentOverviewWindow
    let table = NSTableView()
    let search = NSSearchField()
    let filter = NSSegmentedControl(labels: ["All agents", "Codex", "Claude", "Unattributed"], trackingMode: .selectOne, target: nil, action: nil)
    let grouping = NSSegmentedControl(labels: ["Repository", "Worktree", "Ungrouped"], trackingMode: .selectOne, target: nil, action: nil)
    let summary = NSTextField(labelWithString: "Discovering agents…")
    let connection = NSTextField(labelWithString: "")
    let detail = AgentDetailView(frame: NSRect(x: 0, y: 0, width: 1192, height: 26))
    let detailScroll = NSScrollView()
    var inspectorKey: String?
    var settingsController: ConnectionSettingsController?
    var machineHealth: MachineHealth?
    var connectionState: [String: Any]?
    var readingConnectionState = false
    var readingHostInventory = false
    let hostScanQueue: OperationQueue = {
        let queue = OperationQueue(); queue.name = "hey-boss.host-scans"; queue.qualityOfService = .utility; queue.maxConcurrentOperationCount = 2; return queue
    }()
    let openProject = NSButton(title: "Open project", target: nil, action: nil)
    let copySession = NSButton(title: "Copy session ID", target: nil, action: nil)
    let emptyTitle = NSTextField(labelWithString: "No agents running")
    let emptyHint = NSTextField(wrappingLabelWithString: "Start a Claude or Codex session, then reopen or refresh this overview.")
    let emptyState = NSStackView()
    let statusItem: NSStatusItem?
    var detailBottomConstraint: NSLayoutConstraint?
    var detailHeightConstraint: NSLayoutConstraint?
    var agentGeneration = 0
    var cachedAgentGeneration = -1
    var cachedStale: [Bool] = []
    var orderedRows: [OverviewRow] = []
    var searchCorpus: [String] = []
    var local: AgentSnapshot? { didSet { agentGeneration &+= 1 } }
    var remote: [String: AgentSnapshot] = [:] { didSet { agentGeneration &+= 1 } }
    var rows: [OverviewRow] = []
    var items: [OverviewItem] = []
    var renderedItems: [OverviewItem] = []
    var renderedExpandedAgents: Set<String> = []
    var collapsedGroups: Set<String> = []
    var knownGroups: Set<String> = []
    var savedExpandedGroups: Set<String> = []
    let expansionDefaults: UserDefaults?
    var expandedAgents: Set<String> = []
    var rowCells: [String: OverviewCell] = [:]
    var rowCellModels: [String: OverviewRow] = [:]
    var rowCellExpanded: [String: Bool] = [:]
    var controlPanels: [String: AgentControlPanel] = [:]
    var rebuildSamples: [Double] = []
    var clickSamples: [Double] = []
    var tableUpdateSamples: [Double] = []
    var fullReloads = 0
    var incrementalUpdates = 0
    let expansionPersistenceQueue = DispatchQueue(label: "hey-boss.expansion-preferences", qos: .utility)
    var scanMilliseconds: Double = 0
    let performanceLabel = NSTextField(labelWithString: "")
    let heading = NSTextField(labelWithString: "Agents")
    var filterDebounce: Timer?
    var timer: Timer?
    var scanning = false
    var scanError: String?
    var hostScans: Set<String> = []
    var hostNextScan: [String: Date] = [:]
    var hostFailures: [String: Int] = [:]
    var cli: String?
    let present: Bool
    init(present: Bool = true, cli: String? = ProcessInfo.processInfo.environment["HEY_BOSS_CLI_PATH"], preferences: UserDefaults? = nil) {
        self.present = present
        self.cli = cli
        expansionDefaults = preferences ?? (present ? UserDefaults(suiteName: "local.hey-boss.overview") : nil)
        savedExpandedGroups = Set(expansionDefaults?.stringArray(forKey: "expandedRepositories") ?? [])
        expandedAgents = Set(expansionDefaults?.stringArray(forKey: "expandedAgents") ?? [])
        window = AgentOverviewWindow(contentRect: NSRect(x: 0, y: 0, width: 1240, height: 760), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = "hey-boss · Agents"
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 1000, height: 560)
        statusItem = present ? NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength) : nil
        super.init()
        window.delegate = self
        window.onFind = { [weak self] in
            guard let self else { return }
            self.window.makeFirstResponder(self.search)
            self.search.selectText(nil)
        }
        let content = OverviewCanvas(frame: window.contentView!.bounds)
        window.contentView = content
        let title = heading
        title.font = .systemFont(ofSize: 22, weight: .semibold)
        summary.font = .systemFont(ofSize: 13)
        summary.textColor = .secondaryLabelColor
        connection.font = .systemFont(ofSize: 12)
        connection.textColor = .secondaryLabelColor
        search.placeholderString = "Search agents, tasks, repositories, hosts…"
        search.delegate = self
        filter.selectedSegment = 0
        filter.target = self
        filter.action = #selector(filtersChanged)
        grouping.selectedSegment = 0
        let refresh = NSButton(title: "Refresh", target: self, action: #selector(refreshNow))
        let settings = NSButton(title: "Machines…", target: self, action: #selector(openSettings))
        openProject.target = self
        openProject.action = #selector(openSelectedProject)
        copySession.target = self
        copySession.action = #selector(copySelectedSession)
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = false
        scroll.autohidesScrollers = true
        scroll.borderType = .noBorder
        table.usesAlternatingRowBackgroundColors = false
        table.style = .inset
        table.rowHeight = 78
        table.autoresizingMask = [.width]
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.floatsGroupRows = false
        table.dataSource = self
        table.delegate = self
        table.headerView = nil
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("overview"))
        column.width = 1150
        column.minWidth = 400
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        scroll.documentView = table
        emptyTitle.font = .systemFont(ofSize: 18, weight: .semibold)
        emptyHint.font = .systemFont(ofSize: 13)
        emptyHint.textColor = .secondaryLabelColor
        emptyHint.alignment = .center
        emptyHint.maximumNumberOfLines = 3
        emptyState.orientation = .vertical
        emptyState.alignment = .centerX
        emptyState.spacing = 9
        emptyState.addArrangedSubview(emptyTitle)
        emptyState.addArrangedSubview(emptyHint)
        emptyState.isHidden = true
        performanceLabel.font = .monospacedDigitSystemFont(ofSize: 10, weight: .regular)
        performanceLabel.textColor = .tertiaryLabelColor
        for view in [title, connection, search, refresh, settings, scroll, emptyState, performanceLabel] {
            view.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(view)
        }
        NSLayoutConstraint.activate([
            title.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 24), title.topAnchor.constraint(equalTo: content.topAnchor, constant: 22),
            performanceLabel.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), performanceLabel.centerYAnchor.constraint(equalTo: search.centerYAnchor),
            connection.leadingAnchor.constraint(equalTo: title.leadingAnchor), connection.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 8), connection.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24),
            refresh.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), refresh.centerYAnchor.constraint(equalTo: title.centerYAnchor),
            settings.trailingAnchor.constraint(equalTo: refresh.leadingAnchor, constant: -10), settings.centerYAnchor.constraint(equalTo: refresh.centerYAnchor),
            search.leadingAnchor.constraint(equalTo: title.leadingAnchor), search.topAnchor.constraint(equalTo: connection.bottomAnchor, constant: 18), search.widthAnchor.constraint(equalToConstant: 300),
            search.trailingAnchor.constraint(lessThanOrEqualTo: performanceLabel.leadingAnchor, constant: -18),
            scroll.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 16), scroll.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -16), scroll.topAnchor.constraint(equalTo: search.bottomAnchor, constant: 14), scroll.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -16),
            emptyState.centerXAnchor.constraint(equalTo: scroll.centerXAnchor), emptyState.centerYAnchor.constraint(equalTo: scroll.centerYAnchor), emptyState.widthAnchor.constraint(lessThanOrEqualTo: scroll.widthAnchor, multiplier: 0.8)
        ])
        openProject.isEnabled = false
        copySession.isEnabled = false
        openProject.isHidden = true
        copySession.isHidden = true
        window.center()
        if let button = statusItem?.button {
            button.image = NSImage(systemSymbolName: "bubble.left.and.bubble.right", accessibilityDescription: "hey-boss · Agent overview")
            button.image?.isTemplate = true
            button.title = ""
            button.imagePosition = .imageLeading
            button.toolTip = "hey-boss · Agent overview"
            activityBadge.frame.origin.x = button.bounds.maxX - activityBadge.frame.width
            activityBadge.autoresizingMask = [.minXMargin, .maxYMargin]
            button.addSubview(activityBadge)
        }
        let menu = statusMenu
        activityMenuItem.isEnabled = false
        menu.addItem(activityMenuItem)
        menu.addItem(.separator())
        inboxMenuItem.action = #selector(showInbox)
        inboxMenuItem.target = self
        menu.addItem(inboxMenuItem)
        issuesMenuItem.action = #selector(showIssues)
        issuesMenuItem.target = self
        menu.addItem(issuesMenuItem)
        menu.addItem(.separator())
        menu.addItem(withTitle: "Agent overview…", action: #selector(show), keyEquivalent: "").target = self
        menu.addItem(withTitle: "Machine Health…", action: #selector(showHealth), keyEquivalent: "").target = self
        menu.addItem(withTitle: "Refresh agents", action: #selector(refreshNow), keyEquivalent: "").target = self
        menu.addItem(.separator())
        menu.addItem(withTitle: "Machines…", action: #selector(openSettings), keyEquivalent: "").target = self
        statusItem?.menu = menu
        rebuild()
    }
    func snapshotJSON() throws -> String {
        let state = OverviewState(observedAt: Date().timeIntervalSince1970,
            grouping: "repository",
            search: search.stringValue,
            filter: "all",
            summary: summary.stringValue, connection: connection.stringValue, scanning: scanning || !hostScans.isEmpty,
            scanError: scanError, local: local, servers: remote.values.sorted { $0.host < $1.host },
            rows: rows, collapsedGroups: collapsedGroups.sorted(), expandedAgents: expandedAgents.sorted(), performance: performanceMetrics(), selectedRow: selectedRow()?.key)
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        encoder.outputFormatting = [.sortedKeys]
        let data = try encoder.encode(state)
        guard data.count <= 3 * 1024 * 1024 else { throw StorageError(description: "Overview snapshot is too large") }
        return String(decoding: data, as: UTF8.self)
    }
    func presentWindow() {
        if present { NSApplication.shared.setActivationPolicy(.regular) }
        window.makeKeyAndOrderFront(nil)
        NSApplication.shared.activate(ignoringOtherApps: true)
    }
    func windowWillClose(_ notification: Notification) {
        if present { NSApplication.shared.setActivationPolicy(.accessory) }
    }
    @objc func show() {
        presentWindow()
        for host in Array(hostNextScan.keys) where hostFailures[host] == nil { hostNextScan.removeValue(forKey: host) }
        refreshNow()
    }
    @objc func openSettings() {
        let config = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".hey-boss/config.json")
        if !NSWorkspace.shared.open(config) { NSLog("Could not open machine config") }
    }
    @objc func showHealth() {
        if machineHealth == nil { machineHealth = MachineHealth(present: present, cli: cli) }
        machineHealth?.show()
    }
    func applyConnectionSettings(_ preferences: ConnectionPreferences, completion: @escaping (String?) -> Void) {
        let candidates = [cli, "/opt/homebrew/bin/hey-boss", "/usr/local/bin/hey-boss"].compactMap { $0 }
        guard let path = candidates.first(where: { FileManager.default.isExecutableFile(atPath: $0) }) else { completion("Install the updated hey-boss CLI to save connection settings."); return }
        DispatchQueue.global(qos: .utility).async {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: path)
            process.arguments = ["companion", "configure", preferences.host, "--vpn-domain", preferences.vpnDomain]
            if !preferences.enabled { process.arguments?.append("--disable") }
            let pipe = Pipe()
            process.standardOutput = pipe; process.standardError = pipe
            var error: String?
            do {
                try process.run()
                DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 30) {
                    if process.isRunning {
                        process.terminate()
                        onMain { completion("Connection setup timed out. Check the connection state before retrying.") }
                        DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 2) { if process.isRunning { Darwin.kill(process.processIdentifier, SIGKILL) } }
                    }
                }
                let output = try pipe.fileHandleForReading.readToEnd() ?? Data()
                process.waitUntilExit()
                if process.terminationStatus != 0 {
                    let message = String(decoding: output.prefix(500), as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
                    error = message.isEmpty ? "Couldn’t apply connection settings. Please try again." : message
                }
            } catch let failure { error = failure.localizedDescription }
            let result = error
            onMain { completion(result) }
        }
    }
    @objc func filtersChanged() { rebuild() }
    @objc func groupingChanged() { grouping.selectedSegment = 0; rebuild() }
    @objc func toggleGroup(_ sender: NSButton) {
        let started = ProcessInfo.processInfo.systemUptime
        defer { recordClick(started) }
        guard items.indices.contains(sender.tag), case let .group(key, _, _, _) = items[sender.tag] else { return }
        if collapsedGroups.contains(key) {
            collapsedGroups.remove(key); savedExpandedGroups.insert(key)
        } else {
            collapsedGroups.insert(key); savedExpandedGroups.remove(key)
        }
        persistExpansion()
        rebuild()
    }
    func recordClick(_ started: Double) {
        clickSamples.append((ProcessInfo.processInfo.systemUptime - started) * 1000)
        if clickSamples.count > 60 { clickSamples.removeFirst() }
    }
    func persistExpansion() {
        guard let preferences = expansionDefaults else { return }
        let groups = savedExpandedGroups.sorted(), agents = expandedAgents.sorted()
        expansionPersistenceQueue.async {
            preferences.set(groups, forKey: "expandedRepositories")
            preferences.set(agents, forKey: "expandedAgents")
        }
    }
    func groupIdentity(_ row: OverviewRow) -> (String, String, String) {
        if let git = row.agent.git {
            let key = git.origin == nil ? "repo:\(row.host):\(git.commonDir)" : "repo:\(git.repositoryId)"
            return (key, agentPathName(git.repositoryRoot), git.origin ?? git.repositoryRoot)
        }
        let path = row.agent.cwd ?? "Unknown directory"
        return ("directory:\(row.host):\(path)", agentPathName(path), row.host + " · " + path)
    }
    func performanceMetrics() -> [String: Double] {
        func p95(_ samples: [Double]) -> Double { let sorted = samples.sorted(); return sorted.isEmpty ? 0 : sorted[min(sorted.count - 1, Int(Double(sorted.count) * 0.95))] }
        return ["rebuild_ms": rebuildSamples.last ?? 0, "p95_rebuild_ms": p95(rebuildSamples), "scan_ms": scanMilliseconds, "row_count": Double(rows.count),
                "p95_click_handler_ms": p95(clickSamples), "p95_table_update_ms": p95(tableUpdateSamples),
                "p95_mouse_event_age_ms": p95(window.eventAgeSamples),
                "full_table_reloads": Double(fullReloads), "incremental_table_updates": Double(incrementalUpdates)]
    }
    func displayIndex(for key: String) -> Int? { items.firstIndex { if case let .agent(row) = $0 { return row.key == key }; return false } }
    func controlTextDidChange(_ obj: Notification) {
        filterDebounce?.invalidate()
        filterDebounce = Timer.scheduledTimer(withTimeInterval: 0.12, repeats: false) { [weak self] _ in self?.rebuild() }
    }
    func validateMenuItem(_ menuItem: NSMenuItem) -> Bool {
        menuItem.action != #selector(refreshNow) || window.isVisible
    }
    @objc func refreshNow() {
        rebuild()
        if present && window.isVisible { loadConnectionState(); scanLocal(); scanConfiguredHosts() }
    }
    func loadConnectionState() {
        guard !readingConnectionState else { return }
        readingConnectionState = true
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let path = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/share/hey-boss/connections.json")
            let state = (try? Data(contentsOf: path)).flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
            onMain { guard let self else { return }; self.readingConnectionState = false; self.connectionState = state; self.rebuild() }
        }
    }
    func scanConfiguredHosts() {
        guard !readingHostInventory else { return }
        readingHostInventory = true
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let inventory = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".hey-boss/config.json")
            let hosts = (try? Data(contentsOf: inventory)).map { overviewSSHHosts($0, excluding: nil) } ?? []
            onMain {
                guard let self else { return }; self.readingHostInventory = false
                guard self.present && self.window.isVisible else { return }
                self.scanHosts(hosts)
            }
        }
    }
    func scanHosts(_ hosts: [String]) {
        for host in hosts {
            guard !hostScans.contains(host), (hostNextScan[host] ?? .distantPast) <= Date() else { continue }
            hostScans.insert(host)
            hostScanQueue.addOperation { [weak self] in
                let process = Process()
                process.executableURL = URL(fileURLWithPath: "/usr/bin/ssh")
                process.arguments = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "ConnectionAttempts=1", "-o", "ServerAliveInterval=5", "-o", "ServerAliveCountMax=1", host,
                    "for scanner in ~/.local/bin/hey-boss-scanner ~/.local/bin/hey-boss /opt/homebrew/bin/hey-boss /usr/local/bin/hey-boss; do if test -x \"$scanner\"; then exec \"$scanner\" agents --json; fi; done; exit 127"]
                let pipe = Pipe()
                process.standardOutput = pipe
                process.standardError = FileHandle.nullDevice
                var snapshot: AgentSnapshot?
                defer { try? pipe.fileHandleForReading.close() }
                do {
                    try process.run()
                    DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 25) {
                        if process.isRunning {
                            process.terminate()
                            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 2) { if process.isRunning { Darwin.kill(process.processIdentifier, SIGKILL) } }
                        }
                    }
                    let output = try readScannerOutput(pipe.fileHandleForReading)
                    process.waitUntilExit()
                    if process.terminationStatus == 0 { snapshot = AgentSnapshot.decode(output, allowClockSkew: true) }
                } catch {
                    if process.isRunning {
                        process.terminate()
                        DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 2) { if process.isRunning { Darwin.kill(process.processIdentifier, SIGKILL) } }
                    }
                }
                onMain {
                    guard let self else { return }
                    self.hostScans.remove(host)
                    if let snapshot {
                        self.hostFailures.removeValue(forKey: host)
                        self.hostNextScan[host] = Date().addingTimeInterval(20)
                        self.receive(AgentSnapshot(host: host, observedAt: snapshot.observedAt, agents: snapshot.agents, warnings: snapshot.warnings))
                    } else {
                        let failures = (self.hostFailures[host] ?? 0) + 1
                        self.hostFailures[host] = failures
                        self.hostNextScan[host] = Date().addingTimeInterval(overviewRetryDelay(failures))
                        self.rebuild()
                    }
                }
            }
        }
        rebuild()
    }
    func scanLocal() {
        guard !scanning else { return }
        let candidates = [cli, "/opt/homebrew/bin/hey-boss", "/usr/local/bin/hey-boss", FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("bin/hey-boss").path].compactMap { $0 }
        guard let path = candidates.first(where: { FileManager.default.isExecutableFile(atPath: $0) }) else {
            scanError = "Agent scanner unavailable; install the updated CLI."
            rebuild()
            return
        }
        scanning = true
        let scanStart = ProcessInfo.processInfo.systemUptime
        rebuild()
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let process = Process()
            process.executableURL = URL(fileURLWithPath: path)
            process.arguments = ["agents", "--json"]
            let pipe = Pipe()
            process.standardOutput = pipe
            process.standardError = FileHandle.nullDevice
            var snapshot: AgentSnapshot?
            var failure: String?
            defer { try? pipe.fileHandleForReading.close() }
            do {
                try process.run()
                DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 20) {
                    if process.isRunning {
                        process.terminate()
                        DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 2) { if process.isRunning { Darwin.kill(process.processIdentifier, SIGKILL) } }
                    }
                }
                let data = try readScannerOutput(pipe.fileHandleForReading)
                process.waitUntilExit()
                if process.terminationStatus == 0 { snapshot = AgentSnapshot.decode(data) }
            } catch {
                failure = (error as? StorageError)?.description
                try? pipe.fileHandleForReading.close()
                if process.isRunning {
                    process.terminate()
                    DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 2) { if process.isRunning { Darwin.kill(process.processIdentifier, SIGKILL) } }
                    process.waitUntilExit()
                }
            }
            onMain {
                guard let self else { return }
                self.scanning = false
                self.scanMilliseconds = (ProcessInfo.processInfo.systemUptime - scanStart) * 1000
                if let snapshot { self.local = snapshot; self.scanError = snapshot.warnings.first }
                else { self.scanError = failure ?? "Scanner failed; local data may be stale." }
                self.rebuild()
            }
        }
    }
    func receive(_ snapshot: AgentSnapshot) {
        // Receipt time comes from the Mac; server clock skew cannot hide a disconnect.
        remote[snapshot.host] = AgentSnapshot(host: snapshot.host, observedAt: Date().timeIntervalSince1970, agents: Array(snapshot.agents.prefix(500)), warnings: snapshot.warnings)
        rebuild()
    }
    func rebuild() {
        let started = ProcessInfo.processInfo.systemUptime
        defer {
            let elapsed = (ProcessInfo.processInfo.systemUptime - started) * 1000
            rebuildSamples.append(elapsed)
            if rebuildSamples.count > 60 { rebuildSamples.removeFirst() }
            performanceLabel.stringValue = String(format: "View %.1f ms · scan %.2f s", elapsed, scanMilliseconds / 1000)
        }
        let selected = selectedRow()?.key
        let now = Date().timeIntervalSince1970
        let servers = remote.values.sorted { $0.host < $1.host }
        let stale = [(local.map { now - $0.observedAt > 30 } ?? false)] + servers.map { now - $0.observedAt > 35 }
        if cachedAgentGeneration != agentGeneration || stale != cachedStale {
            var all: [OverviewRow] = []
            if let local { all += local.agents.map { OverviewRow(agent: $0, host: "This Mac", local: true, stale: stale[0]) } }
            for (index, snapshot) in servers.enumerated() { all += snapshot.agents.map { OverviewRow(agent: $0, host: snapshot.host, local: false, stale: stale[index + 1]) } }
            all.removeAll { $0.unattributed }
            // Decorate once: path parsing and localized comparisons are expensive on every keystroke.
            orderedRows = all.map { (row: $0, project: $0.project, working: !$0.stale && $0.agent.state == "Working") }.sorted {
                if $0.working != $1.working { return $0.working }
                let project = $0.project.localizedStandardCompare($1.project)
                if project != .orderedSame { return project == .orderedAscending }
                let host = $0.row.host.localizedStandardCompare($1.row.host)
                if host != .orderedSame { return host == .orderedAscending }
                return ($0.row.agent.pid, $0.row.key) < ($1.row.agent.pid, $1.row.key)
            }.map { $0.row }
            searchCorpus = orderedRows.map { row in
                [row.agent.kind, row.agent.cwd ?? "", row.agent.task ?? "", row.agent.title ?? "", row.agent.activity ?? "", row.agent.update ?? "", row.agent.git?.origin ?? "", row.agent.git?.branch ?? "", row.agent.git?.worktree ?? "", row.host].joined(separator: " ").lowercased()
            }
            cachedAgentGeneration = agentGeneration; cachedStale = stale
        }
        let all = orderedRows
        summary.stringValue = "\(all.count) sessions"
        heading.stringValue = "Agents · \(all.count)"
        window.title = "Agent overview · \(all.count) sessions"
        statusItem?.button?.title = ""
        let state = connectionState
        let machines = state?["machines"] as? [[String: Any]] ?? []
        connection.stringValue = machines.isEmpty ? "No machines connected · Edit Machines… to configure hosts" : machines.compactMap { machine in
            guard let host = machine["host"] as? String else { return nil }
            let phase = machine["state"] as? String ?? "starting"
            let description: String
            switch phase {
            case "connected": description = "connected"
            case "waiting-for-vpn", "waiting-for-stable-vpn": description = host.hasSuffix(".local") ? "waiting for network" : "waiting for VPN"
            case "cooldown": description = "retrying later"
            default: description = phase.replacingOccurrences(of: "-", with: " ")
            }
            return "\(host) · \(description)"
        }.joined(separator: "     ")
        if let error = state?["config_error"] as? String { connection.stringValue += " · Config error: " + error }
        if !hostScans.isEmpty { connection.stringValue += " · Collecting: " + hostScans.sorted().joined(separator: ", ") }
        if !hostFailures.isEmpty {
            connection.stringValue += " · " + hostFailures.keys.sorted().map { host in
                let seconds = max(0, Int((hostNextScan[host] ?? Date()).timeIntervalSinceNow.rounded(.up)))
                return "\(host) unavailable; refresh again in \(seconds)s"
            }.joined(separator: " · ")
        }

        let query = search.stringValue.lowercased()
        rows = query.isEmpty ? all : zip(all, searchCorpus).compactMap { row, text in text.contains(query) ? row : nil }
        items = []
        do {
            var groups: [String: [OverviewRow]] = [:]
            for row in rows { groups[groupIdentity(row).0, default: []].append(row) }
            let orderedGroups = groups.compactMap { key, group -> (key: String, rows: [OverviewRow], identity: (String, String, String), working: Bool)? in
                guard let first = group.first else { return nil }
                return (key: key, rows: group, identity: groupIdentity(first), working: group.contains { !$0.stale && $0.agent.state == "Working" })
            }.sorted {
                if $0.working != $1.working { return $0.working }
                let order = $0.identity.1.localizedStandardCompare($1.identity.1)
                return order == .orderedSame ? $0.key < $1.key : order == .orderedAscending
            }
            for entry in orderedGroups {
                let key = entry.key
                let group = entry.rows
                let identity = entry.identity
                if knownGroups.insert(key).inserted && !savedExpandedGroups.contains(key) { collapsedGroups.insert(key) }
                items.append(.group(key: key, title: identity.1, subtitle: identity.2, count: group.count))
                if !collapsedGroups.contains(key) { items += group.map(OverviewItem.agent) }
            }
        }
        emptyState.isHidden = !rows.isEmpty
        emptyTitle.stringValue = all.isEmpty ? (scanning ? "Looking for agents…" : "No agents running") : "No matching agents"
        emptyHint.stringValue = all.isEmpty ? "Start a Claude or Codex session, then reopen or refresh this overview." : "Try a different search or select All agents."
        if items != renderedItems || expandedAgents != renderedExpandedAgents {
            let updateStarted = ProcessInfo.processInfo.systemUptime
            let old = renderedItems
            if old.isEmpty || table.numberOfRows != old.count {
                table.reloadData(); fullReloads += 1
            } else if items != old {
                var prefix = 0, suffix = 0
                while prefix < min(old.count, items.count), old[prefix] == items[prefix] { prefix += 1 }
                while suffix < min(old.count, items.count) - prefix,
                      old[old.count - suffix - 1] == items[items.count - suffix - 1] { suffix += 1 }
                NSAnimationContext.runAnimationGroup { context in
                    context.duration = 0; context.allowsImplicitAnimation = false
                    table.beginUpdates()
                    if old.count - suffix > prefix { table.removeRows(at: IndexSet(integersIn: prefix..<(old.count - suffix)), withAnimation: []) }
                    if items.count - suffix > prefix { table.insertRows(at: IndexSet(integersIn: prefix..<(items.count - suffix)), withAnimation: []) }
                    table.endUpdates()
                }
                if prefix > 0, items.indices.contains(prefix - 1), case .group = items[prefix - 1] {
                    table.reloadData(forRowIndexes: IndexSet(integer: prefix - 1), columnIndexes: IndexSet(integer: 0))
                }
                incrementalUpdates += 1
            } else {
                let changed = expandedAgents.symmetricDifference(renderedExpandedAgents)
                let indexes = IndexSet(items.indices.filter { if case let .agent(row) = items[$0] { return changed.contains(row.key) }; return false })
                table.noteHeightOfRows(withIndexesChanged: indexes)
                table.reloadData(forRowIndexes: indexes, columnIndexes: IndexSet(integer: 0))
                incrementalUpdates += 1
            }
            renderedItems = items; renderedExpandedAgents = expandedAgents
            let visible = table.rows(in: table.visibleRect)
            if visible.location != NSNotFound, visible.length > 0 {
                for index in visible.location..<NSMaxRange(visible) {
                    if let cell = table.view(atColumn: 0, row: index, makeIfNecessary: false) as? OverviewCell {
                        for button in cell.rowButtons { button.tag = index }
                    }
                }
            }
            tableUpdateSamples.append((ProcessInfo.processInfo.systemUptime - updateStarted) * 1000)
            if tableUpdateSamples.count > 60 { tableUpdateSamples.removeFirst() }
        }
        if let selected, let index = displayIndex(for: selected) { table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false) }
        updateSelection()
    }
    func selectedRow() -> OverviewRow? {
        if items.indices.contains(table.selectedRow), case let .agent(row) = items[table.selectedRow] { return row }
        return nil
    }
    func numberOfRows(in tableView: NSTableView) -> Int { items.count }
    func tableView(_ tableView: NSTableView, isGroupRow row: Int) -> Bool { guard items.indices.contains(row) else { return false }; if case .group = items[row] { return true }; return false }
    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { guard items.indices.contains(row) else { return false }; if case .group = items[row] { return false }; return true }
    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
        guard items.indices.contains(row) else { return 112 }
        if case .group = items[row] { return 42 }
        if case let .agent(agent) = items[row], expandedAgents.contains(agent.key) { return agent.agent.kind.lowercased() == "codex" && agent.agent.sessionId != nil ? 390 : 252 }
        return 112
    }
    @objc func toggleAgent(_ sender: NSButton) {
        let started = ProcessInfo.processInfo.systemUptime
        defer { recordClick(started) }
        guard items.indices.contains(sender.tag), case let .agent(row) = items[sender.tag] else { return }
        if expandedAgents.contains(row.key) { expandedAgents.remove(row.key) } else { expandedAgents.insert(row.key) }
        persistExpansion()
        rebuild()
    }
    @objc func openRowProject(_ sender: NSButton) {
        guard items.indices.contains(sender.tag), case let .agent(row) = items[sender.tag], row.local, let cwd = row.agent.cwd else { return }
        NSWorkspace.shared.open(URL(fileURLWithPath: cwd))
    }
    @objc func copyRowSession(_ sender: NSButton) {
        guard items.indices.contains(sender.tag), case let .agent(row) = items[sender.tag], let id = row.agent.sessionId else { return }
        NSPasteboard.general.clearContents(); NSPasteboard.general.setString(id, forType: .string)
    }
    func controlPanel(for row: OverviewRow) -> AgentControlPanel {
        if let panel = controlPanels[row.key] { return panel }
        let panel = AgentControlPanel()
        panel.perform = { [weak self] action, input, completion in
            guard let self, let session = row.agent.sessionId,
                  session.count == 36, session.allSatisfy({ $0.isHexDigit || $0 == "-" }) else {
                completion(["ok": false, "error": "Session identity unavailable."]); return
            }
            let cliPath = self.cli ?? "/opt/homebrew/opt/hey-boss/libexec/bin/hey-boss"
            DispatchQueue.global(qos: .userInitiated).async {
                let process = Process(), output = Pipe(), stdin = Pipe()
                process.standardOutput = output; process.standardInput = stdin; process.standardError = FileHandle.nullDevice
                if row.local {
                    process.executableURL = URL(fileURLWithPath: cliPath)
                    process.arguments = ["agent-control", "--thread", session, action]
                } else {
                    process.executableURL = URL(fileURLWithPath: "/usr/bin/ssh")
                    process.arguments = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "ConnectionAttempts=1", "-o", "ServerAliveInterval=5", "-o", "ServerAliveCountMax=1", row.host,
                        "for controller in ~/.local/bin/hey-boss-scanner ~/.local/bin/hey-boss /opt/homebrew/opt/hey-boss/libexec/bin/hey-boss /opt/homebrew/bin/hey-boss /usr/local/bin/hey-boss; do if test -x \"$controller\"; then exec \"$controller\" agent-control --thread \(session) \(action); fi; done; exit 127"]
                }
                var response: [String: Any] = ["ok": false, "error": "Control connection unavailable. The remote machine may need the updated CLI and owning-server configuration."]
                defer { try? output.fileHandleForReading.close(); try? stdin.fileHandleForWriting.close() }
                do {
                    let data = try JSONSerialization.data(withJSONObject: input)
                    try process.run()
                    DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 48) {
                        if process.isRunning { Darwin.kill(process.processIdentifier, SIGKILL) }
                    }
                    try stdin.fileHandleForWriting.write(contentsOf: data); try stdin.fileHandleForWriting.close()
                    let bytes = try readScannerOutput(output.fileHandleForReading)
                    process.waitUntilExit()
                    if process.terminationStatus == 0, let result = try JSONSerialization.jsonObject(with: bytes) as? [String: Any] { response = result }
                } catch {
                    response = ["ok": false, "error": "Control connection failed. Check its state before retrying a pending action."]
                    if process.isRunning { Darwin.kill(process.processIdentifier, SIGKILL); process.waitUntilExit() }
                }
                let result = response
                onMain { completion(result) }
            }
        }
        if controlPanels.count >= 200,
           let key = controlPanels.first(where: { !$0.value.busy && $0.value.editor.string.isEmpty && !expandedAgents.contains($0.key) })?.key {
            controlPanels.removeValue(forKey: key)
        }
        controlPanels[row.key] = panel
        return panel
    }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row index: Int) -> NSView? {
        guard items.indices.contains(index) else { return nil }
        if case let .group(key, title, subtitle, count) = items[index] {
            let cell = OverviewCell()
            let disclosure = OverviewDisclosureButton(image: NSImage(systemSymbolName: collapsedGroups.contains(key) ? "chevron.right" : "chevron.down", accessibilityDescription: "Collapse or expand group") ?? NSImage(size: NSSize(width: 16, height: 16)), target: self, action: #selector(toggleGroup))
            disclosure.isBordered = false
            disclosure.tag = index
            let label = NSTextField(labelWithString: "\(title)  ·  \(count) \(count == 1 ? "agent" : "agents")")
            label.font = .systemFont(ofSize: 17, weight: .semibold)
            let location = NSTextField(labelWithString: subtitle)
            location.font = .systemFont(ofSize: 11)
            location.textColor = .secondaryLabelColor
            location.lineBreakMode = .byTruncatingMiddle
            location.toolTip = subtitle
            for view in [disclosure, label, location] { view.translatesAutoresizingMaskIntoConstraints = false; cell.addSubview(view) }
            NSLayoutConstraint.activate([
                disclosure.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 6), disclosure.centerYAnchor.constraint(equalTo: cell.centerYAnchor), disclosure.widthAnchor.constraint(equalToConstant: 16),
                label.leadingAnchor.constraint(equalTo: disclosure.trailingAnchor, constant: 8), label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                location.leadingAnchor.constraint(equalTo: label.trailingAnchor, constant: 14), location.centerYAnchor.constraint(equalTo: cell.centerYAnchor), location.trailingAnchor.constraint(lessThanOrEqualTo: cell.trailingAnchor, constant: -12)
            ])
            let hitTarget = OverviewDisclosureButton(title: "", target: self, action: #selector(toggleGroup))
            hitTarget.isBordered = false
            hitTarget.tag = index
            hitTarget.toolTip = collapsedGroups.contains(key) ? "Expand repository" : "Collapse repository"
            hitTarget.setAccessibilityLabel("\(title), \(count) agents, \(collapsedGroups.contains(key) ? "collapsed" : "expanded")")
            disclosure.setAccessibilityElement(false)
            hitTarget.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(hitTarget)
            cell.rowButtons.append(hitTarget)
            NSLayoutConstraint.activate([
                hitTarget.leadingAnchor.constraint(equalTo: cell.leadingAnchor), hitTarget.trailingAnchor.constraint(equalTo: cell.trailingAnchor),
                hitTarget.topAnchor.constraint(equalTo: cell.topAnchor), hitTarget.bottomAnchor.constraint(equalTo: cell.bottomAnchor)
            ])
            return cell
        }
        guard case let .agent(row) = items[index] else { return nil }
        let expanded = expandedAgents.contains(row.key)
        if let cell = rowCells[row.key], rowCellModels[row.key] == row, rowCellExpanded[row.key] == expanded {
            for button in cell.rowButtons { button.tag = index }
            return cell
        }
        let cell = OverviewCell()
        let toggle = OverviewDisclosureButton(image: NSImage(systemSymbolName: expanded ? "chevron.down" : "chevron.right", accessibilityDescription: expanded ? "Collapse agent" : "Expand agent") ?? NSImage(), target: self, action: #selector(toggleAgent))
        toggle.isBordered = false
        toggle.tag = index
        toggle.toolTip = expanded ? "Hide details" : "Show session, PID and discovery details"
        cell.rowButtons.append(toggle)
        func badge(_ text: String, color: NSColor = .secondaryLabelColor) -> NSTextField {
            let field = NSTextField(labelWithString: " " + text + " ")
            field.font = .systemFont(ofSize: 10, weight: .medium)
            field.textColor = color
            field.drawsBackground = true
            field.backgroundColor = color.withAlphaComponent(0.09)
            field.wantsLayer = true
            field.layer?.cornerRadius = 4
            field.layer?.masksToBounds = true
            return field
        }
        var badges: [NSView] = [badge(row.agent.kind), badge(row.host)]
        if let branch = row.agent.git?.branch { badges.append(badge(branch)) }
        badges.append(badge(row.stateLabel, color: !row.stale && row.agent.state == "Working" ? .systemGreen : .secondaryLabelColor))
        let badgeLine = NSStackView(views: badges)
        badgeLine.orientation = .horizontal; badgeLine.spacing = 6; badgeLine.alignment = .centerY
        func label(_ text: String, font: NSFont, color: NSColor, lines: Int) -> NSTextField {
            let field = NSTextField(wrappingLabelWithString: text)
            field.font = font; field.textColor = color; field.maximumNumberOfLines = lines
            field.lineBreakMode = .byWordWrapping
            (field.cell as? NSTextFieldCell)?.truncatesLastVisibleLine = true
            cell.track(field)
            return field
        }
        let title = label(row.chatLabel, font: .systemFont(ofSize: 13, weight: .semibold), color: .labelColor, lines: 1)
        let progressText = row.agent.update ?? row.agent.task ?? (row.unattributed ? "Live process; no usable session metadata." : "No public progress message yet.")
        let progress = label(progressText, font: .systemFont(ofSize: 12), color: .secondaryLabelColor, lines: expanded ? 3 : 2)
        let activity = label([row.agent.activity ?? "Activity unavailable", agentRelativeTime(row.agent.activityAt)].compactMap { $0 }.joined(separator: " · "), font: .systemFont(ofSize: 11), color: .tertiaryLabelColor, lines: 1)
        var lines: [NSView] = [badgeLine, title, progress, activity]
        if expanded {
            let details = ["Task: \(row.taskLabel)", "Directory: \(row.agent.cwd ?? "unavailable")", "Worktree: \(row.agent.git?.worktree ?? row.agent.cwd ?? "unavailable")", "Session: \(row.agent.sessionId ?? "unavailable") · PID: \(row.agent.pid)", "Discovery: \(row.agent.evidence)"].joined(separator: "\n")
            let field = label(details, font: .systemFont(ofSize: 11), color: .secondaryLabelColor, lines: 7)
            field.isSelectable = true
            lines.append(field)
            var actions: [NSView] = []
            if row.local, row.agent.cwd != nil {
                let button = NSButton(title: "Open folder", target: self, action: #selector(openRowProject)); button.controlSize = .small; button.tag = index
                cell.rowButtons.append(button); actions.append(button)
            }
            if row.agent.sessionId != nil {
                let button = NSButton(title: "Copy session", target: self, action: #selector(copyRowSession)); button.controlSize = .small; button.tag = index
                cell.rowButtons.append(button); actions.append(button)
            }
            if !actions.isEmpty { let bar = NSStackView(views: actions); bar.orientation = .horizontal; bar.spacing = 8; lines.append(bar) }
            if row.agent.kind.lowercased() == "codex", row.agent.sessionId != nil {
                let panel = controlPanel(for: row)
                panel.removeFromSuperview()
                lines.append(panel)
            }
        }
        let body = NSStackView(views: lines)
        body.orientation = .vertical; body.alignment = .leading; body.spacing = 4
        for view in [toggle, body] { view.translatesAutoresizingMaskIntoConstraints = false; cell.addSubview(view) }
        NSLayoutConstraint.activate([
            toggle.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 8), toggle.topAnchor.constraint(equalTo: cell.topAnchor, constant: 9), toggle.widthAnchor.constraint(equalToConstant: 18), toggle.heightAnchor.constraint(equalToConstant: 18),
            body.leadingAnchor.constraint(equalTo: toggle.trailingAnchor, constant: 10), body.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -14), body.topAnchor.constraint(equalTo: cell.topAnchor, constant: 9), body.bottomAnchor.constraint(lessThanOrEqualTo: cell.bottomAnchor, constant: -8),
            title.widthAnchor.constraint(equalTo: body.widthAnchor), progress.widthAnchor.constraint(equalTo: body.widthAnchor), activity.widthAnchor.constraint(equalTo: body.widthAnchor)
        ])
        if let panel = lines.compactMap({ $0 as? AgentControlPanel }).first { panel.widthAnchor.constraint(equalTo: body.widthAnchor).isActive = true }
        for field in lines.compactMap({ $0 as? NSTextField }) { field.widthAnchor.constraint(equalTo: body.widthAnchor).isActive = true }
        if rowCells.count >= 200, let key = rowCells.keys.first { rowCells.removeValue(forKey: key); rowCellModels.removeValue(forKey: key); rowCellExpanded.removeValue(forKey: key) }
        rowCells[row.key] = cell; rowCellModels[row.key] = row; rowCellExpanded[row.key] = expanded
        return cell
    }
    func tableViewSelectionDidChange(_ notification: Notification) { updateSelection() }
    func updateSelection() {
        openProject.isEnabled = false
        copySession.isEnabled = false
        openProject.isHidden = true
        copySession.isHidden = true
    }
    @objc func openSelectedProject() { if let row = selectedRow(), row.local, let cwd = row.agent.cwd { NSWorkspace.shared.open(URL(fileURLWithPath: cwd)) } }
    @objc func copySelectedSession() { if let id = selectedRow()?.agent.sessionId { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(id, forType: .string) } }
}

// Versioned request/reply actions share the authenticated notification transport.
// Only enumerated actions run; websites are opened in the user's normal browser.
final class ActionHTTP: NSObject, URLSessionDataDelegate, URLSessionTaskDelegate {
    var bytes = Data()
    var response: HTTPURLResponse?
    var session: URLSession?
    var done = false
    let completion: (Result<[String: Any], Error>) -> Void
    init(_ request: URLRequest, completion: @escaping (Result<[String: Any], Error>) -> Void) {
        self.completion = completion
        super.init()
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 5
        config.timeoutIntervalForResource = 6
        session = URLSession(configuration: config, delegate: self, delegateQueue: nil)
        session?.dataTask(with: request).resume()
    }
    func finish(_ result: Result<[String: Any], Error>) {
        guard !done else { return }; done = true
        completion(result)
        session?.invalidateAndCancel(); session = nil
    }
    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse, completionHandler: @escaping (URLSession.ResponseDisposition) -> Void) {
        guard let http = response as? HTTPURLResponse, response.expectedContentLength <= 131072 else {
            completionHandler(.cancel); finish(.failure(StorageError(description: "Website response exceeds 128 KiB"))); return
        }
        self.response = http; completionHandler(.allow)
    }
    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        guard bytes.count + data.count <= 131072 else { finish(.failure(StorageError(description: "Website response exceeds 128 KiB"))); return }
        bytes.append(data)
    }
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
        completionHandler(nil) // Return redirect headers; never escape the selected origin.
    }
    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        if let error { finish(.failure(error)); return }
        guard let response else { finish(.failure(StorageError(description: "Website did not return HTTP"))); return }
        var result: [String: Any] = ["status": response.statusCode, "headers": response.allHeaderFields.reduce(into: [String: String]()) { $0[String(describing: $1.key)] = String(describing: $1.value) }]
        if let text = String(data: bytes, encoding: .utf8) { result["body"] = text; result["encoding"] = "utf-8" }
        else { result["body"] = bytes.base64EncodedString(); result["encoding"] = "base64" }
        finish(.success(result))
    }
}

final class DesktopActions {
    struct Site { let generation: String?; let owner: String; let url: URL; let host: String?; let remotePort: Int?; let localPort: Int? }
    let queue = DispatchQueue(label: "hey-boss.desktop-actions")
    var sites: [String: Site] = [:]
    var completed: [String: (String, [String: Any], Bool)] = [:]
    var completionOrder: [String] = []
    var inFlight: Set<String> = []
    var requests: [String: ActionHTTP] = [:]
    var openWebsite: (URL, @escaping (Bool) -> Void) -> Void = { url, completion in onMain { completion(NSWorkspace.shared.open(url)) } }
    var forward: (String, Int) throws -> Int
    var cancel: (String, Int, Int) -> Void
    init(cli: String?) {
        forward = { host, port in
            guard let cli else { throw StorageError(description: "Companion CLI unavailable") }
            let value = try DesktopActions.command(cli, ["companion", "forward", host, String(port)])
            guard let number = Int(value.trimmingCharacters(in: .whitespacesAndNewlines)), number > 0, number <= 65535 else { throw StorageError(description: "Invalid browser forward") }
            return number
        }
        cancel = { host, local, remote in if let cli { _ = try? DesktopActions.command(cli, ["companion", "cancel-forward", host, String(local), String(remote)]) } }
    }
    static func command(_ executable: String, _ args: [String]) throws -> String {
        let file = FileManager.default.temporaryDirectory.appendingPathComponent("hey-boss-action-" + UUID().uuidString)
        guard FileManager.default.createFile(atPath: file.path, contents: nil) else { throw StorageError(description: "Cannot capture companion reply") }
        defer { try? FileManager.default.removeItem(at: file) }
        let output = try FileHandle(forWritingTo: file); defer { try? output.close() }
        let process = Process(); process.executableURL = URL(fileURLWithPath: executable); process.arguments = args
        process.standardOutput = output; process.standardError = FileHandle.nullDevice
        let ended = DispatchSemaphore(value: 0); process.terminationHandler = { _ in ended.signal() }
        try process.run()
        if ended.wait(timeout: .now() + 3) == .timedOut { Darwin.kill(process.processIdentifier, SIGKILL); throw StorageError(description: "Companion action timed out") }
        guard process.terminationStatus == 0 else { throw StorageError(description: "Browser tunnel unavailable; reconnect the companion") }
        let input = try FileHandle(forReadingFrom: file); defer { try? input.close() }
        return String(decoding: try input.read(upToCount: 1024) ?? Data(), as: UTF8.self)
    }
    func handle(_ request: Request, send: @escaping ([String: Any]) -> Void) {
        queue.async { self.route(request, send: send) }
    }
    func route(_ request: Request, send: @escaping ([String: Any]) -> Void) {
        let payload = request.question ?? ""
        guard payload.utf8.count <= 262144, let data = payload.data(using: .utf8),
              let envelope = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let id = envelope["id"] as? String, !id.isEmpty, id.utf8.count <= 128,
              let method = envelope["method"] as? String, let params = envelope["params"] as? [String: Any],
              let version = envelope["version"] as? Int, version == 1 else {
            send(["task_id":"action", "status":"error", "result":"{\"version\":1,\"error\":{\"code\":-32600,\"message\":\"Invalid action envelope or unsupported version\"}}"]); return
        }
        let owner = request.bridge_host ?? request.source_host ?? "This Mac"
        let key = owner + ":" + (request.bridge_generation ?? "local") + ":" + id
        if request.bridge_host != nil {
            let stale = sites.filter { $0.value.owner == owner && $0.value.generation != request.bridge_generation }.map { $0.key }
            for id in stale { sites.removeValue(forKey: id) }
        }
        func emit(_ result: Result<[String: Any], Error>) {
            self.queue.async {
                let body: [String: Any]; let success: Bool
                switch result {
                case .success(let value): body = ["version":1,"id":id,"result":value]; success = true
                case .failure(let error): body = ["version":1,"id":id,"error":["code":-32000,"message":(error as? StorageError)?.description ?? error.localizedDescription]]; success = false
                }
                self.inFlight.remove(key); self.requests.removeValue(forKey: key)
                if success {
                    self.completed[key] = (payload, body, success); self.completionOrder.append(key)
                    while self.completionOrder.count > 32 { self.completed.removeValue(forKey: self.completionOrder.removeFirst()) }
                }
                self.send(body, id: id, success: success, reply: send)
            }
        }
        if let cached = completed[key] {
            if cached.0 == payload { self.send(cached.1, id: id, success: cached.2, reply: send) }
            else { self.send(["version":1,"id":id,"error":["code":-32600,"message":"Request ID reused with different payload"]], id: id, success: false, reply: send) }
            return
        }
        guard inFlight.count < 16, !inFlight.contains(key) else {
            self.send(["version":1,"id":id,"error":["code":-32002,"message":"Action already running or action limit reached"]], id: id, success: false, reply: send); return
        }
        inFlight.insert(key)
        do {
            switch method {
            case "capabilities": emit(.success(["methods":["capabilities","browser.open","browser.request","browser.close"],"protocol":1,"http_response_limit":131072,"browser":"system default","requests":"HTTP to selected website origin"] ))
            case "browser.open":
                guard sites.count < 32, let raw = params["url"] as? String, raw.utf8.count <= 8192,
                      var url = URLComponents(string: raw), ["http","https"].contains(url.scheme?.lowercased() ?? ""),
                      let hostname = url.host, !hostname.isEmpty, url.user == nil, url.password == nil, url.url != nil else { throw StorageError(description: "Expected an HTTP(S) website URL; 32 sessions maximum") }
                var port: Int?; var local: Int?; var host: String?
                if ["localhost","127.0.0.1","::1"].contains(hostname.lowercased()), let bridge = request.bridge_host {
                    let remote = url.port ?? (url.scheme == "https" ? 443 : 80)
                    guard remote > 0, remote <= 65535 else { throw StorageError(description: "Invalid website port") }
                    let forwarded = try forward(bridge, remote)
                    host = bridge; port = remote; local = forwarded; url.host = "127.0.0.1"; url.port = forwarded
                }
                guard let target = url.url else { throw StorageError(description: "Invalid website URL") }
                let session = UUID().uuidString.lowercased()
                let site = Site(generation: request.bridge_generation, owner: owner, url: target, host: host, remotePort: port, localPort: local)
                sites[session] = site // Reserve capacity while the browser opens.
                openWebsite(target) { opened in self.queue.async {
                    guard self.sites[session] != nil else { emit(.failure(StorageError(description: "Connection changed while opening website; open again"))); return }
                    if opened { self.sites[session] = site; emit(.success(["session":session,"url":target.absoluteString,"source_url":raw,"opened":true])) }
                    else { self.sites.removeValue(forKey: session); if let h = host, let p = port, let l = local { self.cancel(h,l,p) }; emit(.failure(StorageError(description: "Default browser could not open website"))) }
                } }
            case "browser.request", "browser.close":
                guard let session = params["session"] as? String, let site = sites[session], site.owner == owner, site.generation == request.bridge_generation else { throw StorageError(description: "Unknown website session for this connection; reopen after app restart") }
                if method == "browser.close" {
                    sites.removeValue(forKey: session)
                    if let h = site.host, let p = site.remotePort, let l = site.localPort { cancel(h,l,p) }
                    emit(.success(["released":true,"session":session])); return
                }
                for field in ["path", "method", "body"] { if let value = params[field], !(value is NSNull), !(value is String) { throw StorageError(description: field + " must be a string") } }
                let path = params["path"] as? String ?? "/"
                let verb = (params["method"] as? String ?? "GET").uppercased()
                guard path.hasPrefix("/"), !path.hasPrefix("//"), path.utf8.count <= 8192,
                      ["GET","POST","PUT","PATCH","DELETE","HEAD","OPTIONS"].contains(verb),
                      var origin = URLComponents(url: site.url, resolvingAgainstBaseURL: false) else { throw StorageError(description: "Expected an origin-relative path and supported HTTP method") }
                origin.path = ""; origin.query = nil; origin.fragment = nil
                guard let base = origin.url, let target = URL(string: path, relativeTo: base)?.absoluteURL,
                      target.host == base.host, target.port == base.port, target.scheme == base.scheme else { throw StorageError(description: "Requests must stay within the selected website origin") }
                var http = URLRequest(url: target); http.httpMethod = verb
                if let body = params["body"] as? String { guard body.utf8.count <= 131072 else { throw StorageError(description: "Request body exceeds 128 KiB") }; http.httpBody = body.data(using: .utf8) }
                if let rawHeaders = params["headers"] {
                    guard let headers = rawHeaders as? [String: String] else { throw StorageError(description: "Headers must be a string-valued object") }
                    let tokens = CharacterSet(charactersIn: "!#$%&'*+-.^_`|~").union(.alphanumerics)
                    guard headers.count <= 32, headers.allSatisfy({ !$0.key.isEmpty && $0.key.utf8.count <= 256 && $0.key.unicodeScalars.allSatisfy { $0.isASCII && tokens.contains($0) } && $0.value.utf8.count <= 8192 && !$0.value.contains(where: { $0.isNewline || $0.isASCII && $0.asciiValue.map { $0 < 32 || $0 == 127 } == true }) && !["host","connection","content-length","transfer-encoding"].contains($0.key.lowercased()) }) else { throw StorageError(description: "Invalid request headers") }
                    for (name,value) in headers { http.setValue(value, forHTTPHeaderField: name) }
                }
                requests[key] = ActionHTTP(http, completion: emit)
            default: throw StorageError(description: "Unsupported desktop action: " + method)
            }
        } catch { emit(.failure(error)) }
    }
    func send(_ body: [String: Any], id: String, success: Bool, reply: ([String: Any]) -> Void) {
        do { let data = try JSONSerialization.data(withJSONObject: body, options: [.sortedKeys]); reply(["task_id":id,"status":success ? "ok" : "error","result":String(decoding:data,as:UTF8.self)]) }
        catch { reply(["task_id":id,"status":"error","result":"{\"error\":{\"message\":\"Cannot encode action response\"}}"] ) }
    }
}

// Match the bundled desktop icon for direct development/audit runs.
// Installed builds use Hey Boss.app and its persistent Launch Services identity.
func applicationIcon() -> NSImage {
    let size = NSSize(width: 512, height: 512)
    let image = NSImage(size: size, flipped: false) { _ in
        NSColor(srgbRed: 0.24, green: 0.29, blue: 0.72, alpha: 1).setFill()
        let tile = NSRect(x: 32, y: 32, width: 448, height: 448)
        NSBezierPath(roundedRect: tile, xRadius: 100, yRadius: 100).fill()
        if let symbol = NSImage(systemSymbolName: "bubble.left.and.bubble.right.fill", accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 266, weight: .medium))?
            .withSymbolConfiguration(.init(paletteColors: [.white])) {
            let width: CGFloat = 302
            let height = width * symbol.size.height / symbol.size.width
            symbol.draw(in: NSRect(x: (512 - width) / 2, y: (512 - height) / 2, width: width, height: height))
        }
        return true
    }
    image.isTemplate = false
    image.accessibilityDescription = "hey-boss"
    return image
}

@main
struct Daemon {
    static func main() {
        if Bundle.main.bundleIdentifier != "local.hey-boss.desktop" {
            NSApplication.shared.applicationIconImage = applicationIcon()
        }
        #if HEY_BOSS_MOBILE_AUDIT
        auditMobile()
        #elseif HEY_BOSS_AGENT_PREVIEW
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        let overview = makeAgentPreview()
        let controller = AgentPreviewController(overview)
        app.mainMenu = controller.menu()
        overview.show()
        withExtendedLifetime((overview, controller)) { app.run() }
        #elseif HEY_BOSS_INBOX_FIXTURE
        runInboxFixture()
        #elseif HEY_BOSS_AUDIT
        audit()
        #else
        signal(SIGPIPE, SIG_IGN)
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let ui = Interface(present: true)
        let overview = AgentsOverview()
        let secrets = SecretPrompts()
        let actions = DesktopActions(cli: ProcessInfo.processInfo.environment["HEY_BOSS_CLI_PATH"])
        guard let directory = ProcessInfo.processInfo.environment["HEY_BOSS_STATE_DIR"] else {
            reportFailure(StorageError(description: "HEY_BOSS_STATE_DIR is missing")); return
        }
        let store: Store
        do { store = try Store(directory + "/history.db") }
        catch { reportFailure(error); return }
        store.mobileRequired = FileManager.default.fileExists(atPath: directory + "/mobile.json")
        store.mobile = MobileHub.load(store: store, directory: directory)
        let presence = MacPresence()
        store.mobile?.presence = { presence.snapshot }
        store.mobile?.onRouting = { routing in onMain { overview.updateActivity(routing) } }
        store.mobile?.start()
        store.show = { row in onMain { ui.add(row) } }
        store.remove = { id in onMain { ui.remove(id) } }
        store.removeMany = { ids in onMain { ui.remove(ids) } }
        ui.onComplete = { id, answer in store.queue.async {
            do { try store.finish(id, answer) }
            catch { reportFailure(error); onMain { ui.retryAnswer(id, message: String(describing: error)) } }
        } }
        ui.saveComment = { id, text, quote, commentID, selection, completion in
            store.queue.async {
                do { let row = try store.addComment(id, text: text, quote: quote, commentID: commentID, selection: selection); onMain { completion(.success(row)) } }
                catch { onMain { completion(.failure(error)) } }
            }
        }
        ui.finishReview = { id, completion in
            store.queue.async {
                do { try store.finish(id, nil); let row = try store.database.get(id); onMain { completion(.success(row)) } }
                catch { onMain { completion(.failure(error)) } }
            }
        }
        ui.onCompleteMany = { ids in store.queue.async { store.complete(ids) } }
        ui.onDismissMany = { ids in store.queue.async { store.dismiss(ids) } }
        ui.onPresented = { id, time in store.queue.async { store.presented(id, time) } }
        store.pendingChanged = { count in onMain { overview.updateInboxCount(count) } }
        store.queue.async { store.restore() }
        var sockets: UnsafeMutablePointer<Int32>?
        var count = 0
        guard launchActivateSocket("Listener", &sockets, &count) == 0, count == 1, let activated = sockets else {
            if let sockets { free(sockets) }
            reportFailure(StorageError(description: "Listener socket is unavailable")); return
        }
        let listener = activated[0]
        free(sockets)
        guard fcntl(listener, F_SETFL, 0) == 0 else { reportFailure(StorageError(description: "Cannot configure listener")); return }
        let readers = DispatchSemaphore(value: 16)
        DispatchQueue.global(qos: .userInitiated).async {
            while true {
                readers.wait()
                let fd = accept(listener, nil, nil)
                if fd < 0 {
                    readers.signal()
                    if errno == EINTR { continue }
                    reportFailure(StorageError(description: "Listener accept failed (errno \(errno))"))
                    return
                }
                DispatchQueue.global(qos: .userInitiated).async {
                    defer { readers.signal() }
                    let reply = Reply(fd)
                    let request: Request
                    do {
                        let data = try readRequest(fd)
                        request = try JSONDecoder().decode(Request.self, from: data)
                    } catch { reply.send(["status": "error", "error": "Invalid request"]); return }
                    if request.command == "protocol" {
                        reply.send(["task_id": "protocol", "status": "ok", "result": "1"])
                    } else if request.command == "secret" {
                        onMain { secrets.handle(request, reply) }
                    } else if request.command == "action" {
                        actions.handle(request) { reply.send($0) }
                    } else if request.command == "overview" {
                        onMain { overview.show() }
                        reply.send(["task_id": "overview", "status": "ok"])
                    } else if request.command == "inbox" || request.command == "issues" {
                        onMain {
                            let item = request.command == "inbox" ? overview.inboxMenuItem : overview.issuesMenuItem
                            let opened = NSApp.sendAction(item.action!, to: item.target, from: item)
                            reply.send(["task_id": request.command, "status": opened ? "ok" : "error"])
                        }
                    } else if request.command == "menu_snapshot" {
                        onMain {
                            let state: [String: Any] = ["items": overview.statusMenu.items.filter { !$0.isSeparatorItem }.map(\.title),
                                "inbox_native": false, "inbox_count": overview.inboxCount,
                                "inbox_url": "http://127.0.0.1:4781/#view=inbox", "issues_launching": overview.issuesLauncher.launching]
                            let data = try? JSONSerialization.data(withJSONObject: state, options: .sortedKeys)
                            reply.send(["task_id": "menu", "status": "ok", "result": String(decoding: data ?? Data(), as: UTF8.self)])
                        }
                    } else if request.command == "health" {
                        onMain { overview.showHealth() }
                        reply.send(["task_id": "health", "status": "ok"])
                    } else if request.command == "overview_snapshot" {
                        onMain {
                            do { reply.send(["task_id": "overview", "status": "ok", "result": try overview.snapshotJSON()]) }
                            catch { reply.send(["task_id": "overview", "status": "error", "error": "Overview snapshot unavailable"]); reportFailure(error) }
                        }
                    } else if request.command == "agents_snapshot" {
                        if let content = request.question, let snapshot = AgentSnapshot.decode(Data(content.utf8), allowClockSkew: true) {
                            onMain { if overview.window.isVisible { overview.receive(snapshot) } }
                            reply.send(["task_id": "agents", "status": "ok"])
                        } else { reply.send(["task_id": "agents", "status": "error"]) }
                    } else { store.queue.async { store.handle(request, reply) } }
                }
            }
        }
        app.run()
        #endif
    }
}
