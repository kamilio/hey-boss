import AppKit
import Carbon

final class StickyShortcut {
    private var key: EventHotKeyRef?
    private var handler: EventHandlerRef?
    let toggle: () -> Void
    private(set) var registered = false
    init(toggle: @escaping () -> Void) {
        self.toggle = toggle
        var type = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        guard InstallEventHandler(GetApplicationEventTarget(), { _, event, context in
            guard let event, let context else { return OSStatus(eventNotHandledErr) }
            var id = EventHotKeyID()
            guard GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), nil, MemoryLayout<EventHotKeyID>.size, nil, &id) == noErr, id.signature == 0x5354494b else { return OSStatus(eventNotHandledErr) }
            Unmanaged<StickyShortcut>.fromOpaque(context).takeUnretainedValue().toggle()
            return noErr
        }, 1, &type, Unmanaged.passUnretained(self).toOpaque(), &handler) == noErr else { return }
        registered = RegisterEventHotKey(UInt32(kVK_ANSI_N), UInt32(controlKey | cmdKey | optionKey | shiftKey), EventHotKeyID(signature: 0x5354494b, id: 1), GetApplicationEventTarget(), 0, &key) == noErr
    }
    deinit {
        if let key { UnregisterEventHotKey(key) }
        if let handler { RemoveEventHandler(handler) }
    }
}

final class StickyApp: NSObject, NSApplicationDelegate {
    private var store: NoteStore!
    private var windows: [String: NoteWindow] = [:]
    private var server: CommandServer?
    private var shortcut: StickyShortcut?
    func applicationDidFinishLaunching(_ notification: Notification) {
        do {
            store = try NoteStore()
            // Acquire the instance lock before reading or writing any notes.
            server = try CommandServer { [weak self] in self?.handle($0) ?? ["error":"Sticky is stopping."] }
            try store.ensureDefault()
            for note in try store.all() { try open(note).show() }
            installMenus()
            shortcut = StickyShortcut { [weak self] in self?.toggleAll() }
            if shortcut?.registered != true { NSLog("Sticky: Hyper+N is unavailable; use sticky toggle.") }
        } catch {
            fputs("Sticky: \(error.localizedDescription)\n", stderr)
            NSApp.terminate(nil)
        }
    }
    private func open(_ note: Note) throws -> NoteWindow {
        if let window = windows[note.id] { return window }
        let window = try NoteWindow(note: note, position: windows.count, save: { [weak self] note in
            guard let self else { throw StickyError("Sticky is stopping.") }
            try self.store.save(note)
        })
        windows[note.id] = window
        return window
    }
    private func handle(_ value: [String: Any]) -> [String: Any] {
        do {
            let command = value["command"] as? String ?? ""
            let id = value["id"] as? String ?? "focus"
            switch command {
            case "status": return ["running":true, "pid":ProcessInfo.processInfo.processIdentifier, "visible":windows.values.filter { $0.window.isVisible }.count, "hotkey":"Control+Command+Option+Shift+N", "hotkey_registered":shortcut?.registered ?? false]
            case "list": return ["notes":try store.all().map { ["id":$0.id, "title":$0.title, "path":try store.url($0.id).path, "visible":windows[$0.id]?.window.isVisible ?? false] }]
            case "get":
                let note = try store.read(id)
                return ["id":id, "title":note.title, "markdown":note.markdown]
            case "edit": try open(store.read(id)).edit()
            case "set", "new":
                guard let markdown = value["markdown"] as? String else { throw StickyError("Missing Markdown.") }
                let note = Note(id: command == "new" ? UUID().uuidString.lowercased() : id, markdown: markdown)
                if windows[note.id]?.dirty == true { throw StickyError("This note has unsaved edits. Save or cancel them before replacing it.") }
                try store.save(note)
                if let window = windows[note.id] { try window.update(note) }
                try open(note).show()
                return ["id":note.id, "title":note.title, "path":try store.url(note.id).path]
            case "show":
                if value["id"] != nil { try open(store.read(id)).show() } else { showAll() }
            case "hide":
                if value["id"] != nil { _ = try store.read(id); windows[id]?.hideNote() } else { hideAll() }
            case "toggle": toggleAll()
            case "quit":
                guard !windows.values.contains(where: { $0.dirty }) else { throw StickyError("Save or cancel unsaved note edits before quitting.") }
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { NSApp.terminate(nil) }
            default: throw StickyError("Unknown command: \(command)")
            }
            return ["ok":true]
        } catch { return ["error":error.localizedDescription] }
    }
    @objc private func showAll() { windows.values.forEach { $0.show() } }
    @objc private func hideAll() { windows.values.forEach { $0.hideNote() } }
    @objc private func toggleAll() {
        if windows.values.contains(where: { $0.window.isVisible }) { hideAll() } else { showAll() }
    }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool { showAll(); return true }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        if windows.values.contains(where: { $0.dirty }) {
            let alert = NSAlert(); alert.messageText = "There are unsaved note edits."; alert.informativeText = "Save or cancel them before quitting Sticky."
            alert.runModal(); return .terminateCancel
        }
        windows.values.forEach { $0.invalidate() }
        return .terminateNow
    }
    private func installMenus() {
        let bar = NSMenu()
        let appItem = NSMenuItem(); let appMenu = NSMenu(title: "Sticky")
        appMenu.addItem(withTitle: "Quit Sticky", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu; bar.addItem(appItem)
        let editItem = NSMenuItem(); let edit = NSMenu(title: "Edit")
        for (title, action, key) in [("Undo","undo:","z"),("Cut","cut:","x"),("Copy","copy:","c"),("Paste","paste:","v"),("Select All","selectAll:","a")] {
            edit.addItem(withTitle: title, action: Selector(action), keyEquivalent: key)
        }
        editItem.submenu = edit; bar.addItem(editItem); NSApp.mainMenu = bar
    }
}
