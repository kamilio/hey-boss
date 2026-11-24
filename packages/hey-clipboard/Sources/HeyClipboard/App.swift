import AppKit
import Carbon
import SwiftUI
import TalkCore

final class ClipboardApp: NSObject, NSApplicationDelegate, ObservableObject {
    let history: History
    let monitor: ClipboardMonitor
    @Published var cycling = false {
        didSet { if !cycling { cycleTimer?.invalidate(); cycleTimer = nil } }
    }
    @Published var paused = false
    @Published var message: String?
    @Published var query = ""
    @Published var selected: UUID?
    @Published var pinsOnly = false
    @Published private(set) var browsing = false
    let server = Server()
    private var timer: Timer?
    private var cycleTimer: Timer?
    private var statusItem: NSStatusItem!
    private var hotkey: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private(set) var hotkeyRegistered = false
    private var picker: PickerPanel?
    private var target: NSRunningApplication?
    private var pasting = false

    init(history: History) {
        self.history = history
        monitor = ClipboardMonitor(history: history)
        super.init()
        server.handle = { [weak self] request, reply in self?.command(request, reply: reply) }
    }
    var visible: [Clip] {
        let rows = history.matching(query)
        return pinsOnly ? rows.filter(\.pinned) : rows
    }
    var selection: Clip? {
        let rows = visible
        return rows.first { $0.id == selected } ?? rows.first
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        statusItem.button?.image = NSImage(systemSymbolName: "clipboard", accessibilityDescription: "Hey Clipboard")
        statusItem.button?.target = self
        statusItem.button?.action = #selector(statusClick)
        statusItem.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])
        installMenus()
        registerHotkey()
        // Build and lay out the small native panel before the first shortcut.
        picker = PickerPanel(model: self)
        picker?.contentView?.layoutSubtreeIfNeeded()
        let timer = Timer(timeInterval: 0.35, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.monitor.poll(app: NSWorkspace.shared.frontmostApplication?.localizedName ?? "Unknown")
        }
        timer.tolerance = 0.1
        RunLoop.main.add(timer, forMode: .common); self.timer = timer
    }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        openPicker(); return true
    }
    func applicationWillTerminate(_ notification: Notification) { history.finishWrites() }

    @objc private func statusClick() {
        if NSApp.currentEvent?.type == .rightMouseUp {
            let menu = NSMenu()
            menu.addItem(withTitle: "Open Clipboard History", action: #selector(openPicker), keyEquivalent: "")
            menu.addItem(withTitle: paused ? "Resume Capture" : "Pause Capture", action: #selector(togglePause), keyEquivalent: "")
            menu.addItem(.separator())
            menu.addItem(withTitle: "Quit Hey Clipboard", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
            for item in menu.items where item.action != #selector(NSApplication.terminate(_:)) { item.target = self }
            statusItem.menu = menu; statusItem.button?.performClick(nil); statusItem.menu = nil
        } else { openPicker() }
    }
    @objc func togglePause() {
        paused.toggle(); monitor.paused = paused
        // Consume changes during the paused interval, so resuming never captures them later.
        if !paused { monitor.paused = true; monitor.poll(app: ""); monitor.paused = false }
        statusItem.button?.image = NSImage(systemSymbolName: paused ? "clipboard.badge.ellipsis" : "clipboard", accessibilityDescription: "Hey Clipboard")
    }
    @objc func openPicker() {
        cycling = false
        showPicker(browsing: true)
    }
    func cycle() {
        if cycling, picker?.isVisible == true { move(1) }
        else { cycling = true; showPicker(browsing: false) }
        guard cycleTimer == nil else { return }
        // Modifier release may happen before the panel becomes key. Reading the
        // current flags needs no global keyboard-monitoring permission.
        let timer = Timer(timeInterval: 0.025, repeats: true) { [weak self] _ in
            self?.flagsChanged(NSEvent.modifierFlags)
        }
        timer.tolerance = 0.005
        RunLoop.main.add(timer, forMode: .common); cycleTimer = timer
    }
    func flagsChanged(_ flags: NSEvent.ModifierFlags) {
        if cycling, flags.intersection([.command, .shift]).isEmpty { pasteSelection() }
    }
    private func showPicker(browsing: Bool) {
        let sameMode = self.browsing == browsing
        self.browsing = browsing
        if sameMode, picker?.isKeyWindow == true, NSApp.isActive { return }
        if let app = NSWorkspace.shared.frontmostApplication, app.processIdentifier != getpid() { target = app }
        query = ""; pinsOnly = false; selected = history.clips.first?.id; message = nil
        if picker == nil { picker = PickerPanel(model: self) }
        // The nonactivating quick picker must stay above the destination's windows.
        picker?.level = browsing ? .normal : .popUpMenu
        picker?.hidesOnDeactivate = browsing
        picker?.setContentSize(NSSize(width: browsing ? 660 : 480, height: browsing ? 560 : 300))
        picker?.centerOnMouseScreen()
        picker?.makeKeyAndOrderFront(nil)
        if browsing { NSApp.activate(ignoringOtherApps: true) }
    }
    func dismiss() { cycling = false; picker?.orderOut(nil); target?.activate() }
    func move(_ delta: Int) {
        let rows = visible
        guard !rows.isEmpty else { selected = nil; return }
        let index = rows.firstIndex { $0.id == selected } ?? 0
        selected = rows[(index + delta + rows.count) % rows.count].id
    }
    func deleteSelection() {
        guard let clip = selection else { return }
        let rows = visible, index = rows.firstIndex { $0.id == clip.id } ?? 0
        history.delete(clip.id)
        selected = visible.isEmpty ? nil : visible[min(index, visible.count - 1)].id
    }
    func copySelection() {
        guard let clip = selection else { return }
        if monitor.copy(clip) { message = "Copied" } else { message = "Couldn’t write to the clipboard." }
    }
    func pasteSelection() {
        cycling = false
        guard let clip = selection, let target else { message = "Choose a destination app, then open the picker again."; return }
        paste(clip, into: target) { result in
            if let error = result["error"] as? String {
                self.message = error
                self.picker?.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            }
        }
    }
    private func paste(_ clip: Clip, into target: NSRunningApplication, reply: @escaping ([String: Any]) -> Void) {
        guard !pasting, !target.isTerminated, target.processIdentifier != getpid() else { reply(["ok": false, "error": "The destination is unavailable or a paste is already in progress."]); return }
        guard monitor.copy(clip) else { reply(["ok": false, "error": "Couldn’t write to the clipboard."]); return }
        pasting = true
        let count = NSPasteboard.general.changeCount
        picker?.orderOut(nil)
        if !target.isActive { target.activate() }
        let deadline = ProcessInfo.processInfo.systemUptime + 0.5
        func postWhenActive() {
            guard target.isActive else {
                guard !target.isTerminated, ProcessInfo.processInfo.systemUptime < deadline else {
                    self.pasting = false
                    reply(["ok": false, "error": "Copied. The destination did not become active; nothing was pasted."])
                    return
                }
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.01, execute: postWhenActive)
                return
            }
            DispatchQueue.global(qos: .userInitiated).async {
                let result: [String: Any]
                do {
                    result = try ControlSocket.request(["command": "paste_clipboard", "pid": target.processIdentifier, "change_count": count], timeout: 3)
                } catch { result = ["ok": false, "error": "Copied. Open Hey Talk to enable pasting, or press Command-V."] }
                DispatchQueue.main.async { self.pasting = false; reply(result) }
            }
        }
        postWhenActive()
    }
    private func registerHotkey() {
        var event = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, _, pointer in
            guard let pointer else { return OSStatus(eventNotHandledErr) }
            Unmanaged<ClipboardApp>.fromOpaque(pointer).takeUnretainedValue().cycle()
            return noErr
        }, 1, &event, Unmanaged.passUnretained(self).toOpaque(), &handler)
        hotkeyRegistered = RegisterEventHotKey(9, UInt32(cmdKey | shiftKey), EventHotKeyID(signature: 0x48434C50, id: 1), GetApplicationEventTarget(), 0, &hotkey) == noErr
        if !hotkeyRegistered { message = "Command+Shift+V is in use. Quit Flycut, then restart Hey Clipboard." }
    }
    private func installMenus() {
        let bar = NSMenu(), app = NSMenu(), edit = NSMenu()
        let appItem = NSMenuItem(), editItem = NSMenuItem()
        appItem.submenu = app; editItem.submenu = edit; bar.addItem(appItem); bar.addItem(editItem)
        app.addItem(withTitle: "Quit Hey Clipboard", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        for (title, selector, key) in [("Cut", "cut:", "x"), ("Copy", "copy:", "c"), ("Paste", "paste:", "v"), ("Select All", "selectAll:", "a")] {
            edit.addItem(withTitle: title, action: Selector(selector), keyEquivalent: key)
        }
        NSApp.mainMenu = bar
    }
    private func command(_ request: [String: Any], reply: @escaping ([String: Any]) -> Void) {
        func success(_ extra: [String: Any] = [:]) { reply(extra.merging(["ok": true]) { _, new in new }) }
        let command = request["command"] as? String ?? ""
        switch command {
        case "status": success(["pid": getpid(), "paused": paused, "entries": history.clips.count, "pinned": history.clips.filter(\.pinned).count,
                                 "captured": monitor.captured, "skipped": monitor.skipped, "shortcut": "Command+Shift+V", "hotkey_registered": hotkeyRegistered, "picker_visible": picker?.isVisible == true, "cycling": cycling,
                                 "warning": history.warning as Any? ?? NSNull()])
        case "list", "search":
            let rows = history.matching(request["query"] as? String ?? "").filter { request["pinned"] as? Bool != true || $0.pinned }
            success(["entries": rows.map(\.summary)])
        case "show": openPicker(); success()
        case "hide": cycling = false; picker?.orderOut(nil); success()
        case "pause", "resume":
            if paused != (command == "pause") { togglePause() }; success(["paused": paused])
        case "clear": history.clear(); success()
        case "quit":
            history.finishWrites(); success()
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { NSApp.terminate(nil) }
        case "import-flycut":
            do {
                let file = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Containers/com.generalarcade.flycut/Data/Library/Preferences/com.generalarcade.flycut.plist")
                let count = try history.importFlycut(Data(contentsOf: file))
                history.finishWrites(); success(["imported": count])
            } catch { reply(["ok": false, "error": error.localizedDescription]) }
        case "copy" where request["text"] is String:
            let text = request["text"] as! String
            guard !text.isEmpty, text.utf8.count <= 1_048_576 else { reply(["ok": false, "error": "Expected nonempty text, at most 1 MiB"]); return }
            let clip = Clip(text: text, app: "CLI")
            guard monitor.copy(clip), let saved = history.clips.first else { reply(["ok": false, "error": "Copy failed"]); return }
            success(["entry": saved.summary])
        case "get", "copy", "paste", "delete", "pin", "unpin":
            guard let raw = request["id"] as? String, let id = UUID(uuidString: raw), let clip = history.clips.first(where: { $0.id == id }) else {
                reply(["ok": false, "error": "Entry not found"]); return
            }
            switch command {
            case "get":
                var entry = clip.summary
                if clip.imageType != nil { entry["image_path"] = history.imageURL(clip).path }
                else { entry["text"] = clip.text }
                success(["entry": entry])
            case "copy":
                if monitor.copy(clip) { success(["entry": clip.summary]) } else { reply(["ok": false, "error": "Copy failed"]) }
            case "delete": history.delete(id); success()
            case "pin", "unpin": history.pin(id, command == "pin"); success()
            default:
                guard let bundle = request["app"] as? String, let app = NSRunningApplication.runningApplications(withBundleIdentifier: bundle).first else { reply(["ok": false, "error": "Destination app is not running"]); return }
                paste(clip, into: app, reply: reply)
            }
        default: reply(["ok": false, "error": "Unknown command"])
        }
    }
}
