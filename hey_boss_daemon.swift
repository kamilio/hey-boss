import AppKit
import Foundation
import SQLite3
import Darwin

@_silgen_name("launch_activate_socket")
func launchActivateSocket(_ name: UnsafePointer<CChar>, _ fds: UnsafeMutablePointer<UnsafeMutablePointer<Int32>?>, _ count: UnsafeMutablePointer<Int>) -> Int32

func onMain(_ action: @escaping () -> Void) {
    RunLoop.main.perform(inModes: [.common], block: action)
    CFRunLoopWakeUp(CFRunLoopGetMain())
}

struct Request: Decodable {
    let command: String
    let question: String?
    let project: String?
    let title: String?
    let description: String?
    let options: [String]?
    let autoclose: Double?
    let link_url: String?
    let link_label: String?
    let task_id: String?
    let sync: Bool
    let origin: LaunchOrigin?
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
    var completedAt: Double?

    var heading: String { "\(project!) · \(title!)" }

    var response: [String: String] {
        var response = ["task_id": taskID, "status": status]
        if let result { response["result"] = result }
        return response
    }
}

final class Reply {
    let channel: DispatchIO
    init(_ fd: Int32) {
        channel = DispatchIO(type: .stream, fileDescriptor: fd, queue: .global(qos: .userInitiated)) { _ in Darwin.close(fd) }
    }
    func send(_ response: [String: String]) {
        let bytes = try! JSONSerialization.data(withJSONObject: response)
        let data = bytes.withUnsafeBytes { DispatchData(bytes: $0) }
        channel.write(offset: 0, data: data, queue: .global(qos: .userInitiated)) { [self] done, _, _ in
            if done { channel.close() }
        }
    }
}

final class Database {
    let db: OpaquePointer
    let saveStatement: OpaquePointer
    let readStatement: OpaquePointer
    init(_ path: String) {
        var pointer: OpaquePointer?
        precondition(sqlite3_open(path, &pointer) == SQLITE_OK)
        db = pointer!
        precondition(sqlite3_exec(db, "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS dialogs (id TEXT PRIMARY KEY, status TEXT NOT NULL, body TEXT NOT NULL); CREATE INDEX IF NOT EXISTS pending ON dialogs(status);", nil, nil, nil) == SQLITE_OK)
        precondition(sqlite3_exec(db, "UPDATE dialogs SET body=json_set(body, '$.title', json_extract(body, '$.description')) WHERE json_extract(body, '$.kind')='update' AND json_extract(body, '$.title') IS NULL;", nil, nil, nil) == SQLITE_OK)
        precondition(sqlite3_exec(db, "CREATE VIEW IF NOT EXISTS notifications AS SELECT id, json_extract(body, '$.project') AS project, json_extract(body, '$.title') AS title, json_extract(body, '$.kind') AS kind, json_extract(body, '$.question') AS message, json_extract(body, '$.description') AS summary, status, json_extract(body, '$.createdAt') AS created_at, json_extract(body, '$.presentedAt') AS presented_at, json_extract(body, '$.completedAt') AS completed_at FROM dialogs;", nil, nil, nil) == SQLITE_OK)
        var save: OpaquePointer?
        precondition(sqlite3_prepare_v2(db, "INSERT INTO dialogs VALUES (?, ?, ?) ON CONFLICT(id) DO UPDATE SET status=excluded.status, body=excluded.body", -1, &save, nil) == SQLITE_OK)
        saveStatement = save!
        var read: OpaquePointer?
        precondition(sqlite3_prepare_v2(db, "SELECT body FROM dialogs WHERE id=?", -1, &read, nil) == SQLITE_OK)
        readStatement = read!
    }
    func save(_ row: Record) {
        let body = String(data: try! JSONEncoder().encode(row), encoding: .utf8)!
        row.taskID.withCString { id in
            row.status.withCString { status in
                body.withCString { text in
                    precondition(sqlite3_bind_text(saveStatement, 1, id, -1, nil) == SQLITE_OK)
                    precondition(sqlite3_bind_text(saveStatement, 2, status, -1, nil) == SQLITE_OK)
                    precondition(sqlite3_bind_text(saveStatement, 3, text, -1, nil) == SQLITE_OK)
                    precondition(sqlite3_step(saveStatement) == SQLITE_DONE)
                    precondition(sqlite3_reset(saveStatement) == SQLITE_OK)
                    precondition(sqlite3_clear_bindings(saveStatement) == SQLITE_OK)
                }
            }
        }
    }
    func get(_ id: String) -> Record {
        id.withCString { text in
            precondition(sqlite3_bind_text(readStatement, 1, text, -1, nil) == SQLITE_OK)
            precondition(sqlite3_step(readStatement) == SQLITE_ROW)
            let body = String(cString: sqlite3_column_text(readStatement, 0))
            let record = try! JSONDecoder().decode(Record.self, from: Data(body.utf8))
            precondition(sqlite3_reset(readStatement) == SQLITE_OK)
            precondition(sqlite3_clear_bindings(readStatement) == SQLITE_OK)
            return record
        }
    }
    func pending() -> [Record] {
        var statement: OpaquePointer?
        precondition(sqlite3_prepare_v2(db, "SELECT body FROM dialogs WHERE status='pending' ORDER BY rowid", -1, &statement, nil) == SQLITE_OK)
        var rows: [Record] = []
        var step = sqlite3_step(statement)
        while step == SQLITE_ROW {
            let body = String(cString: sqlite3_column_text(statement, 0))
            rows.append(try! JSONDecoder().decode(Record.self, from: Data(body.utf8)))
            step = sqlite3_step(statement)
        }
        precondition(step == SQLITE_DONE)
        precondition(sqlite3_finalize(statement) == SQLITE_OK)
        return rows
    }
}

final class Store {
    let queue = DispatchQueue(label: "hey-boss.store", qos: .userInitiated)
    let database: Database
    var waiters: [String: [Reply]] = [:]
    var show: ((Record) -> Void)!
    var remove: ((String) -> Void)!
    var removeMany: (([String]) -> Void)!
    init(_ path: String) { database = Database(path) }
    func restore() {
        for row in database.pending() {
            if let expiry = row.expiresAt, expiry <= Date().timeIntervalSince1970 {
                complete(row.taskID, nil)
            } else {
                show(row)
            }
        }
    }
    func handle(_ request: Request, _ reply: Reply) {
        if ["alert", "update", "ask"].contains(request.command) {
            let isAlert = request.command == "alert"
            let isUpdate = request.command == "update"
            let options = request.command == "ask" ? request.options! : []
            precondition(!request.project!.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            precondition(!request.title!.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            if let autoclose = request.autoclose { precondition(autoclose.isFinite && autoclose > 0) }
            precondition((request.link_url == nil) == (request.link_label == nil))
            if let link = request.link_url { precondition(["https", "http", "file"].contains(URL(string: link)!.scheme!)) }
            let row = Record(taskID: UUID().uuidString.lowercased(), kind: isUpdate ? "update" : (isAlert ? "alert" : (options.isEmpty ? "prompt" : "approval")), question: request.question!, project: request.project, title: request.title, description: isAlert ? "" : request.description!, options: options, autoclose: request.autoclose, linkURL: request.link_url, linkLabel: request.link_label, createdAt: Date().timeIntervalSince1970, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: request.origin)
            database.save(row)
            if request.sync { waiters[row.taskID] = [reply] }
            else { reply.send(["task_id": row.taskID]) }
            show(row)
        } else {
            let row = database.get(request.task_id!)
            switch request.command {
            case "status": reply.send(row.response)
            case "hide":
                precondition(row.kind == "alert" || row.kind == "update")
                complete(row.taskID, nil)
                reply.send(["task_id": row.taskID, "status": "ok"])
            case "wait":
                precondition(row.kind == "prompt" || row.kind == "approval")
                if row.status == "ok" { reply.send(row.response) }
                else if waiters[row.taskID] != nil { waiters[row.taskID]!.append(reply) }
                else { waiters[row.taskID] = [reply] }
            default: preconditionFailure(request.command)
            }
        }
    }
    func presented(_ id: String, _ timestamp: Double) {
        var row = database.get(id)
        if row.status == "ok" || row.presentedAt != nil { return }
        row.presentedAt = timestamp
        if let seconds = row.autoclose { row.expiresAt = timestamp + seconds }
        database.save(row)
    }
    func complete(_ ids: [String]) {
        let rows = ids.map { database.get($0) }
        precondition(rows.allSatisfy { $0.kind == "alert" || $0.kind == "update" })
        let timestamp = Date().timeIntervalSince1970
        precondition(sqlite3_exec(database.db, "BEGIN IMMEDIATE", nil, nil, nil) == SQLITE_OK)
        for var row in rows where row.status == "pending" {
            row.status = "ok"
            row.completedAt = timestamp
            database.save(row)
        }
        precondition(sqlite3_exec(database.db, "COMMIT", nil, nil, nil) == SQLITE_OK)
        removeMany(ids)
    }
    func complete(_ id: String, _ result: String?) {
        var row = database.get(id)
        if row.status == "ok" { return }
        if row.kind == "approval" { precondition(row.options.contains(result!)) }
        if row.kind == "prompt" { precondition(result != nil) }
        row.status = "ok"
        row.completedAt = Date().timeIntervalSince1970
        row.result = result
        database.save(row)
        if let replies = waiters.removeValue(forKey: id) {
            for reply in replies { reply.send(row.response) }
        }
        remove(id)
    }
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
            let parsed = try! AttributedString(markdown: line, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))
            for run in parsed.runs {
                let text = String(parsed[run.range].characters)
                var runFont = font
                if let intent = run.inlinePresentationIntent {
                    if intent.contains(.stronglyEmphasized) { runFont = .systemFont(ofSize: font.pointSize, weight: .bold) }
                    if intent.contains(.emphasized) { runFont = NSFontManager.shared.convert(runFont, toHaveTrait: .italicFontMask) }
                    if intent.contains(.code) { runFont = .monospacedSystemFont(ofSize: size - 1, weight: .regular) }
                }
                var attributes: [NSAttributedString.Key: Any] = [.font: runFont, .foregroundColor: color, .paragraphStyle: paragraph]
                if let link = run.link {
                    precondition(["https", "http", "file"].contains(link.scheme!))
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
    func place(_ proposed: NSRect, display: Bool) {
        var target = proposed
        if let anchor = dragAnchor {
            let area = visibleArea
            target.origin = NSPoint(
                x: min(max(anchor.x, area.minX), area.maxX - target.width),
                y: min(max(anchor.y - target.height, area.minY), area.maxY - target.height)
            )
        }
        placing = true
        setFrame(target, display: display)
        placing = false
    }
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
    override func sendEvent(_ event: NSEvent) {
        if event.type == .keyDown && event.isARepeat && [36, 76].contains(event.keyCode) { return }
        if event.type == .keyDown && event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command {
            let selectors = ["a": "selectAll:", "c": "copy:", "v": "paste:", "x": "cut:", "z": "undo:"]
            if let selector = selectors[event.charactersIgnoringModifiers!] {
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
    required init?(coder: NSCoder) { fatalError() }
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
    var invoke: (() -> Void)!
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
    required init?(coder: NSCoder) { fatalError() }
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
        image = NSImage(systemSymbolName: "info.circle", accessibilityDescription: "Launch details")
        toolTip = "Launch details"
        setAccessibilityLabel("Launch details")
        invoke = { [weak self] in self!.showInfo() }
    }
    required init?(coder: NSCoder) { fatalError() }
    func showInfo() {
        let popover = NSPopover()
        popover.behavior = .transient
        popover.contentViewController = infoView(row)
        self.popover = popover
        popover.show(relativeTo: bounds, of: self, preferredEdge: .minY)
    }
}

final class Surface: NSView {
    let content = NSView()
    let glass = NSGlassEffectView()
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
        }
    }
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer!.cornerRadius = 22
        layer!.cornerCurve = .continuous
        layer!.masksToBounds = true
        layer!.backgroundColor = NSColor.black.withAlphaComponent(0.01).cgColor
        glass.frame = bounds
        glass.autoresizingMask = [.width, .height]
        glass.cornerRadius = 22
        glass.style = .regular
        glass.wantsLayer = true
        glass.layer!.cornerRadius = 22
        glass.layer!.cornerCurve = .continuous
        glass.layer!.masksToBounds = true
        content.frame = bounds
        content.autoresizingMask = [.width, .height]
        glass.contentView = content
        addSubview(glass)
    }
    required init?(coder: NSCoder) { fatalError() }
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

final class Preview: NSWindow, NSWindowDelegate, NSTextViewDelegate {
    var onClose: (() -> Void)!
    let text = DocumentText()
    let openURL: (URL) -> Void
    var linkButtons: [ActionButton] = []
    init(_ row: Record, openURL: @escaping (URL) -> Void) {
        self.openURL = openURL
        super.init(contentRect: NSRect(x: 0, y: 0, width: 720, height: 640), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        title = row.title!
        subtitle = row.project!
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
        accessory.view = InfoButton(row, frame: NSRect(x: 0, y: 0, width: 32, height: 28))
        addTitlebarAccessoryViewController(accessory)
        center()
    }
    func windowWillClose(_ notification: Notification) { onClose() }
    func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
        openURL(link as! URL)
        return true
    }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let modifiers = event.modifierFlags.intersection([.control, .command, .option, .shift])
        let key = event.charactersIgnoringModifiers?.lowercased()
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
    var link: ActionButton?
    var timer: Timer?
    var projectLabel: DragHeader!
    var info: InfoButton!
    var contentHeight: CGFloat!
    var grouped: Bool?
    init(_ row: Record, open: @escaping () -> Void, openURL: @escaping (URL) -> Void, complete: @escaping (String, String?) -> Void) {
        self.row = row
        if row.kind == "update" {
            view = effect(NSRect(x: 0, y: 0, width: 344, height: 116))
            let header = DragHeader(labelWithString: row.title!)
            header.font = .systemFont(ofSize: 13, weight: .semibold)
            header.textColor = .labelColor
            header.lineBreakMode = .byTruncatingTail
            header.frame = NSRect(x: 18, y: 84, width: 280, height: 20)
            header.toolTip = row.heading
            view.content.addSubview(header)
            let summary = PlainTextField(wrappingLabelWithString: row.description)
            summary.font = .systemFont(ofSize: 13, weight: .regular)
            summary.textColor = .labelColor
            summary.maximumNumberOfLines = 1
            summary.lineBreakMode = .byTruncatingTail
            summary.frame = NSRect(x: 18, y: 60, width: 308, height: 20)
            summary.toolTip = row.description
            view.content.addSubview(summary)
            let button = ActionButton("Read update", frame: NSRect(x: 188, y: 12, width: 138, height: 36), style: .secondary) {
                open()
                complete(row.taskID, nil)
            }
            button.alignment = .center
            button.imagePosition = .imageTrailing
            button.toolTip = "Open Markdown preview"
            button.setAccessibilityLabel("Read \(row.project!) update: \(row.title!)")
            view.content.addSubview(button)
            link = button
            close = ActionButton("", frame: NSRect(x: 304, y: 78, width: 32, height: 32), style: .quiet) { complete(row.taskID, nil) }
            close.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Dismiss update")
            close.setAccessibilityLabel("Dismiss \(row.project!) update: \(row.title!)")
            view.content.addSubview(close)
            info = InfoButton(row, frame: NSRect(x: 272, y: 78, width: 32, height: 32))
            view.content.addSubview(info)
            prepareProjectLabel()
            return
        }
        let body = label(row.question, width: 308, size: 14, color: .labelColor)
        let footer: CGFloat = row.linkURL == nil ? 0 : 54
        let height = 56 + body.frame.height + footer
        view = effect(NSRect(x: 0, y: 0, width: 344, height: height))
        body.setFrameOrigin(NSPoint(x: 18, y: 18 + footer))
        view.content.addSubview(body)
        let header = DragHeader(labelWithString: row.title!)
        header.toolTip = row.heading
        header.lineBreakMode = .byTruncatingTail
        header.font = .systemFont(ofSize: 11, weight: .medium)
        header.textColor = secondaryTextColor()
        header.frame = NSRect(x: 18, y: height - 36, width: 244, height: 28)
        view.content.addSubview(header)
        close = ActionButton("", frame: NSRect(x: 304, y: height - 38, width: 32, height: 32), style: .quiet) { complete(row.taskID, nil) }
        close.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Dismiss notification")
        close.contentTintColor = .secondaryLabelColor
        close.setAccessibilityLabel("Dismiss \(row.project!) notification: \(row.title!)")
        view.content.addSubview(close)
        info = InfoButton(row, frame: NSRect(x: 272, y: height - 38, width: 32, height: 32))
        view.content.addSubview(info)
        prepareProjectLabel()
        if let url = row.linkURL {
            let button = ActionButton(row.linkLabel!, frame: NSRect(x: 0, y: 12, width: min(264, max(136, (row.linkLabel! as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 13, weight: .medium)]).width + 48)), height: 36), style: .secondary) {
                openURL(URL(string: url)!)
                complete(row.taskID, nil)
            }
            button.frame.origin.x = 326 - button.frame.width
            button.cell!.lineBreakMode = .byTruncatingTail
            button.toolTip = row.linkLabel!
            button.setAccessibilityLabel("\(row.linkLabel!) for \(row.project!): \(row.title!)")
            button.image = NSImage(systemSymbolName: "arrow.up.right", accessibilityDescription: nil)
            button.imagePosition = .imageTrailing
            view.content.addSubview(button)
            link = button
        }
    }
    func prepareProjectLabel() {
        contentHeight = view.frame.height
        projectLabel = DragHeader(labelWithString: row.project!)
        projectLabel.font = .systemFont(ofSize: 12, weight: .semibold)
        projectLabel.textColor = .labelColor
        projectLabel.lineBreakMode = .byTruncatingTail
        projectLabel.toolTip = row.project!
        projectLabel.frame = NSRect(x: 18, y: contentHeight - 2, width: 244, height: 20)
        view.content.addSubview(projectLabel)
    }
    func configure(grouped: Bool) {
        if self.grouped == grouped { return }
        self.grouped = grouped
        projectLabel.isHidden = grouped
        view.setFrameSize(NSSize(width: 344, height: contentHeight + (grouped ? 0 : 26)))
        view.drawsSurface = !grouped
        close.frame.origin.y = view.frame.height - 38
        info.frame.origin = row.kind == "alert" && row.linkURL == nil ? NSPoint(x: 272, y: view.frame.height - 38) : NSPoint(x: 12, y: 14)
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
    init(_ project: String, toggle: @escaping () -> Void, clear: @escaping () -> Void) {
        self.project = project
        count.font = .monospacedDigitSystemFont(ofSize: 11, weight: .semibold)
        count.textColor = secondaryTextColor()
        count.alignment = .right
        summary.font = .systemFont(ofSize: 12, weight: .medium)
        summary.textColor = .labelColor
        summary.lineBreakMode = .byTruncatingTail
        detail.font = .systemFont(ofSize: 12)
        detail.textColor = secondaryTextColor()
        detail.lineBreakMode = .byTruncatingTail
        self.toggle = ActionButton(project, frame: .zero, style: .quiet, action: toggle)
        self.toggle.font = .systemFont(ofSize: 13, weight: .semibold)
        self.toggle.contentTintColor = .labelColor
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
        let height: CGFloat = expanded ? 44 + cards.reduce(CGFloat(0)) { $0 + $1.view.frame.height + 1 } : 86
        view.setFrameSize(NSSize(width: 344, height: height))
        count.stringValue = "\(cards.count)"
        count.frame = NSRect(x: 256, y: height - 30, width: 28, height: 18)
        toggle.frame = NSRect(x: 12, y: height - 38, width: 236, height: 32)
        toggle.image = NSImage(systemSymbolName: expanded ? "chevron.down" : "chevron.right", accessibilityDescription: expanded ? "Collapse project" : "Expand project")
        toggle.setAccessibilityLabel("\(expanded ? "Collapse" : "Expand") \(project), \(cards.count) notifications")
        toggle.toolTip = "\(expanded ? "Collapse" : "Expand") \(project)"
        clear.setAccessibilityLabel("Clear all \(cards.count) \(project) notifications")
        clear.frame = NSRect(x: 300, y: height - 38, width: 36, height: 32)
        summary.stringValue = cards[0].row.title!
        summary.toolTip = cards[0].row.title!
        summary.frame = NSRect(x: 44, y: 28, width: 280, height: 18)
        summary.isHidden = expanded
        detail.stringValue = cards[0].row.kind == "update" ? cards[0].row.description : markdown(cards[0].row.question, size: 12, color: secondaryTextColor()).string.replacingOccurrences(of: "\n", with: " ")
        detail.toolTip = detail.stringValue
        detail.frame = NSRect(x: 44, y: 10, width: 280, height: 16)
        detail.isHidden = expanded
        while dividers.count > cards.count { dividers.removeLast().removeFromSuperview() }
        while dividers.count < cards.count {
            let divider = NSBox()
            divider.boxType = .separator
            view.content.addSubview(divider)
            dividers.append(divider)
        }
        var y = height - 44
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

final class Interface {
    let present: Bool
    let stack = panel("Hey Boss notifications")
    let scroll = NSScrollView()
    let document = NSView()
    let glassContainer = NSGlassEffectContainerView()
    let question = panel("Hey Boss question")
    var cards: [Card] = []
    var groupHeaders: [NSView] = []
    var projectGroups: [String: ProjectGroup] = [:]
    var expandedProjects: Set<String> = []
    var questions: [Record] = []
    var current: Record?
    var field: NSTextField?
    var buttons: [ActionButton] = []
    var onComplete: ((String, String?) -> Void)!
    var onCompleteMany: (([String]) -> Void)!
    var onPresented: ((String, Double) -> Void)!
    var observer: NSObjectProtocol?
    var previews: [String: Preview] = [:]
    var openURL: (URL) -> Void = { precondition(NSWorkspace.shared.open($0)) }
    init(present: Bool) {
        self.present = present
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        glassContainer.contentView = document
        glassContainer.spacing = 0
        scroll.documentView = glassContainer
        stack.contentView = scroll
        stack.hasShadow = false
        observer = NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in self!.layout() }
    }
    func add(_ row: Record) {
        if row.kind == "alert" || row.kind == "update" {
            let card = Card(row, open: { [weak self] in self!.openPreview(row) }, openURL: openURL) { [weak self] id, answer in self!.onComplete(id, answer) }
            cards.append(card)
            document.addSubview(card.view)
            layout()
            if present { stack.orderFrontRegardless() }
            let now = Date().timeIntervalSince1970
            onPresented(row.taskID, now)
            if let seconds = row.autoclose {
                let deadline: Double
                if let expires = row.expiresAt { deadline = expires }
                else { deadline = now + seconds }
                card.timer = Timer.scheduledTimer(withTimeInterval: max(0.001, deadline - now), repeats: false) { [weak self] _ in self!.onComplete(row.taskID, nil) }
            }
        } else {
            questions.append(row)
            nextQuestion()
        }
    }
    func openPreview(_ row: Record) {
        if previews[row.taskID] == nil {
            let preview = Preview(row, openURL: openURL)
            preview.onClose = { [weak self] in self!.previews.removeValue(forKey: row.taskID) }
            previews[row.taskID] = preview
        }
        if present {
            previews[row.taskID]!.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        }
    }
    func remove(_ ids: [String]) {
        let removed = Set(ids)
        for card in cards where removed.contains(card.row.taskID) { card.view.removeFromSuperview() }
        cards.removeAll { removed.contains($0.row.taskID) }
        layout()
    }
    func remove(_ id: String) {
        if let index = cards.firstIndex(where: { $0.row.taskID == id }) {
            cards[index].view.removeFromSuperview()
            cards.remove(at: index)
            layout()
        } else if current?.taskID == id {
            current = nil
            question.orderOut(nil)
            question.contentView = nil
            buttons = []
            field = nil
            nextQuestion()
        } else {
            questions.removeAll { $0.taskID == id }
        }
    }
    func layout() {
        let screen = stack.visibleArea
        groupHeaders = []
        let groupedProjects = Set(Dictionary(grouping: cards, by: { $0.row.project! }).filter { $0.value.count >= 3 }.keys)
        for project in Array(projectGroups.keys) where !groupedProjects.contains(project) {
            projectGroups.removeValue(forKey: project)!.view.removeFromSuperview()
        }
        if cards.isEmpty { stack.orderOut(nil); return }
        let groups = Dictionary(grouping: cards, by: { $0.row.project! }).map { project, cards in
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
                return total + group.cards.reduce(CGFloat(0)) { $0 + $1.view.frame.height + 10 }
            }
            return total + (expandedProjects.contains(group.project) ? 44 + group.cards.reduce(CGFloat(0)) { $0 + $1.view.frame.height + 1 } : 86) + 10
        }
        let height = min(total, screen.height - 24)
        let gutter = total > height && scroll.scrollerStyle == .legacy ? NSScroller.scrollerWidth(for: .regular, scrollerStyle: .legacy) : 0
        let oldTop = document.frame.height - scroll.contentView.bounds.maxY
        stack.place(NSRect(x: screen.minX + 12, y: screen.maxY - height - 12, width: 344 + gutter, height: height), display: present)
        scroll.frame = NSRect(x: 0, y: 0, width: 344 + gutter, height: height)
        glassContainer.frame = NSRect(x: 0, y: 0, width: 344, height: total)
        document.frame = glassContainer.bounds
        var y = total
        for group in groups {
            if group.cards.count < 3 {
                for card in group.cards {
                    if card.view.superview !== document { document.addSubview(card.view) }
                    y -= card.view.frame.height
                    card.view.setFrameOrigin(NSPoint(x: 0, y: y))
                    y -= 10
                }
                continue
            }
            let expanded = expandedProjects.contains(group.project)
            let project = group.project
            if projectGroups[project] == nil {
                projectGroups[project] = ProjectGroup(project, toggle: { [weak self] in
                    if self!.expandedProjects.contains(project) { self!.expandedProjects.remove(project) }
                    else { self!.expandedProjects.insert(project) }
                    self!.layout()
                }, clear: { [weak self] in
                    self!.onCompleteMany(self!.cards.filter { $0.row.project == project }.map { $0.row.taskID })
                })
            }
            let projectView = projectGroups[group.project]!
            projectView.update(group.cards, expanded: expanded)
            y -= projectView.view.frame.height
            if projectView.view.superview !== document { document.addSubview(projectView.view) }
            projectView.view.setFrameOrigin(NSPoint(x: 0, y: y))
            groupHeaders.append(projectView.view)
            y -= 10
        }
        scroll.contentView.scroll(to: NSPoint(x: 0, y: max(0, total - height - max(0, oldTop))))
        scroll.reflectScrolledClipView(scroll.contentView)
    }
    func nextQuestion() {
        if current != nil || questions.isEmpty { return }
        let row = questions.removeFirst()
        current = row
        question.title = row.heading
        buttons = []
        field = nil
        let title = label(row.question, width: 432, size: 17, color: .labelColor)
        let description = label(row.description, width: 432, size: 13, color: secondaryTextColor())
        let optionWidths = row.options.map { max(88, ($0 as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 13, weight: .medium)]).width + 32) }
        let optionsWidth = optionWidths.reduce(0, +) + CGFloat(max(0, row.options.count - 1)) * 8
        let inlineOptions = optionsWidth <= 432
        let optionHeights = row.options.map {
            max(44, ceil(($0 as NSString).boundingRect(with: NSSize(width: 384, height: CGFloat.greatestFiniteMagnitude), options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: [.font: NSFont.systemFont(ofSize: 13, weight: .medium)]).height) + 20)
        }
        let inputHeight: CGFloat = row.kind == "prompt" || inlineOptions ? 48 : optionHeights.reduce(CGFloat(0)) { $0 + $1 + 8 }
        let natural = 108 + title.frame.height + description.frame.height + inputHeight
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
        header.frame = NSRect(x: 24, y: natural - 44, width: 392, height: 28)
        body.addSubview(header)
        body.addSubview(InfoButton(row, frame: NSRect(x: 424, y: natural - 44, width: 32, height: 28)))
        title.setFrameOrigin(NSPoint(x: 24, y: natural - 58 - title.frame.height))
        body.addSubview(title)
        description.setFrameOrigin(NSPoint(x: 24, y: title.frame.minY - 8 - description.frame.height))
        body.addSubview(description)
        if row.kind == "prompt" {
            let input = PlainTextField(frame: NSRect(x: 24, y: 24, width: 318, height: 40))
            input.controlSize = .large
            input.bezelStyle = .roundedBezel
            input.font = .systemFont(ofSize: 14)
            input.placeholderString = "Your answer"
            let inputHeight = input.intrinsicContentSize.height
            input.frame = NSRect(x: 24, y: 44 - inputHeight / 2, width: 318, height: inputHeight)
            input.setAccessibilityLabel(row.question)
            body.addSubview(input)
            field = input
            let submit = ActionButton("Submit", frame: NSRect(x: 350, y: 22, width: 106, height: 44), style: .primary) { [weak self, weak input] in self!.answer(input!.stringValue) }
            submit.keyEquivalent = "\r"
            input.target = submit
            input.action = #selector(ActionButton.activate)
            body.addSubview(submit)
            buttons = [submit]
        } else {
            var x = 456 - optionsWidth
            var optionY = 20 + inputHeight
            for (index, option) in row.options.enumerated() {
                optionY -= optionHeights[index] + 8
                let frame = inlineOptions ? NSRect(x: x, y: 22, width: optionWidths[index], height: 44) : NSRect(x: 24, y: optionY, width: 432, height: optionHeights[index])
                let button = ActionButton(option, frame: frame, style: .secondary) { [weak self] in self!.answer(option) }
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
        question.contentView = view
        question.place(NSRect(x: screen.midX - 240, y: screen.midY - height / 2, width: 480, height: height), display: present)
        bodyScroll.contentView.scroll(to: NSPoint(x: 0, y: max(0, natural - height)))
        bodyScroll.reflectScrolledClipView(bodyScroll.contentView)
        if present {
            question.makeKeyAndOrderFront(nil)
            if let field { question.makeFirstResponder(field) }
        }
        onPresented(row.taskID, Date().timeIntervalSince1970)
    }
    func answer(_ text: String) {
        for button in buttons { button.isEnabled = false }
        field?.isEnabled = false
        onComplete(current!.taskID, text)
    }
}

@main
struct Daemon {
    static func main() {
        #if HEY_BOSS_AUDIT
        audit()
        #else
        signal(SIGPIPE, SIG_IGN)
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let ui = Interface(present: true)
        let directory = ProcessInfo.processInfo.environment["HEY_BOSS_STATE_DIR"]!
        let store = Store(directory + "/history.db")
        store.show = { row in onMain { ui.add(row) } }
        store.remove = { id in onMain { ui.remove(id) } }
        store.removeMany = { ids in onMain { ui.remove(ids) } }
        ui.onComplete = { id, answer in store.queue.async { store.complete(id, answer) } }
        ui.onCompleteMany = { ids in store.queue.async { store.complete(ids) } }
        ui.onPresented = { id, time in store.queue.async { store.presented(id, time) } }
        store.queue.async { store.restore() }
        var sockets: UnsafeMutablePointer<Int32>?
        var count = 0
        precondition(launchActivateSocket("Listener", &sockets, &count) == 0)
        precondition(count == 1)
        let listener = sockets![0]
        free(sockets)
        precondition(fcntl(listener, F_SETFL, 0) == 0)
        DispatchQueue.global(qos: .userInitiated).async {
            while true {
                let fd = accept(listener, nil, nil)
                precondition(fd >= 0)
                DispatchQueue.global(qos: .userInitiated).async {
                    let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: false)
                    let data = try! handle.readToEnd()!
                    let request = try! JSONDecoder().decode(Request.self, from: data)
                    let reply = Reply(fd)
                    store.queue.async { store.handle(request, reply) }
                }
            }
        }
        app.run()
        #endif
    }
}
