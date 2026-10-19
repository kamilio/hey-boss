import AppKit
import AVFoundation
import TalkCore

final class TalkService: NSObject, NSApplicationDelegate {
    private let server = CommandServer()
    private let capture = AudioCapture()
    private let inference = Inference()
    private let history = DictationHistory()
    private var historyWindow: HistoryWindow?
    private lazy var cues = AudioCues()
    private var session = RecordingSession()
    private var overlay: Overlay?
    private var hotkey: Hotkey?
    private var menuItem: NSStatusItem?
    private var pendingReply: (([String: Any]) -> Void)?
    private var pendingStartReply: (([String: Any]) -> Void)?
    private var target: NSRunningApplication?
    private var paste = true
    private var transcriptionSource = "microphone"
    private var overlayEnabled: Bool
    private var error: String?
    private var captureReady = false
    private var captureMilliseconds: Double = 0
    private var idleRelease: DispatchWorkItem?
    private var maximumDuration: DispatchWorkItem?
    private var signals: [DispatchSourceSignal] = []
    private var inputUID: String? { UserDefaults.standard.string(forKey: "inputUID") }

    init(headless: Bool) { overlayEnabled = !headless; super.init() }
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        do { try server.start() }
        catch { fputs("hey-talk: \(error.localizedDescription)\n", stderr); NSApp.terminate(nil); return }
        server.handle = { [weak self] request, reply in self?.command(request, reply: reply) }
        capture.onFailure = { [weak self] message in self?.fail(message) }
        hotkey = Hotkey()
        hotkey?.onAction = { [weak self] action in
            guard let self else { return }
            switch action {
            case .start: self.start(showOverlay: self.overlayEnabled, paste: true) { _ in }
            case .stop: if self.session.phase == .recording { self.stop { _ in } }
            case .cancel: if self.session.phase == .recording || self.session.phase == .processing { self.cancel() }
            case .none: break
            }
        }
        menuItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        menuItem?.button?.image = NSImage(systemSymbolName: "waveform", accessibilityDescription: "Hey Talk")
        let menu = NSMenu()
        for (title, action) in [("Dictation History…", #selector(showHistory)), ("Start / Stop Dictation", #selector(toggle)), ("Cancel Dictation", #selector(cancelMenu)), ("Show / Hide Overlay", #selector(toggleOverlay)), ("Quit Hey Talk", #selector(quit))] {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: ""); item.target = self; menu.addItem(item)
        }
        menuItem?.menu = menu
        history.refresh()
        if overlayEnabled { showOverlay(true) }
        for number in [SIGTERM, SIGINT] {
            signal(number, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: number, queue: .main)
            source.setEventHandler { NSApp.terminate(nil) }; source.resume(); signals.append(source)
        }
        NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main) { [weak self] _ in self?.cancel() }
    }
    func applicationWillTerminate(_ notification: Notification) { capture.cancel(); inference.cancel(); history.finishPendingWrites() }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
    @objc private func showHistory() {
        if historyWindow == nil { historyWindow = HistoryWindow(history: history) }
        history.refresh(); historyWindow?.show()
    }
    private func showOverlay(_ show: Bool) {
        if overlay == nil && show {
            let view = Overlay()
            view.onToggle = { [weak self] in self?.toggle() }
            view.onCancel = { [weak self] in self?.cancel() }
            view.level = { [weak self] in self?.capture.level ?? 0 }
            overlay = view
        }
        overlayEnabled = show; overlay?.show(show)
    }
    @objc private func toggleOverlay() { showOverlay(!overlayEnabled) }
    @objc private func toggle() {
        if session.phase == .recording { stop { _ in } }
        else if session.phase != .processing { start(showOverlay: overlayEnabled, paste: true) { _ in } }
    }
    @objc private func cancelMenu() { cancel() }
    @objc private func quit() { NSApp.terminate(nil) }

    private func status() -> [String: Any] {
        var value: [String: Any] = [
            "ok": true, "state": session.phase.rawValue, "pid": getpid(),
            "model": "orukeet-v0.1.0", "model_ready": ModelPaths.available,
            "hotkey": DictationShortcut.name, "hotkey_registered": hotkey?.registered ?? false,
            "escape_registered": hotkey?.escapeRegistered ?? false, "hotkey_events": hotkey?.eventsReceived ?? 0,
            "overlay": overlayEnabled, "input": inputUID ?? "default",
            "sample_count": capture.sampleCount, "rms": capture.level,
            "capture_start_ms": captureMilliseconds, "peak_rms": capture.maximumLevel,
            "microphone_ready": captureReady,
            "microphone": permissionName, "accessibility": AXIsProcessTrusted(),
        ]
        if let pid = inference.pid { value["inference_pid"] = pid }
        if let error { value["last_error"] = error }
        return value
    }
    private var permissionName: String {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: return "granted"
        case .denied: return "denied"
        case .restricted: return "restricted"
        case .notDetermined: return "not_determined"
        @unknown default: return "unknown"
        }
    }
    private func command(_ request: [String: Any], reply: @escaping ([String: Any]) -> Void) {
        let command = request["command"] as? String ?? ""
        switch command {
        case "status", "doctor": reply(status())
        case "start": start(showOverlay: request["overlay"] as? Bool ?? overlayEnabled, paste: request["paste"] as? Bool ?? true, reply: reply)
        case "stop": stop(reply)
        case "toggle":
            if session.phase == .recording { stop(reply) }
            else { start(showOverlay: request["overlay"] as? Bool ?? overlayEnabled, paste: request["paste"] as? Bool ?? true, reply: reply) }
        case "cancel": cancel(); reply(status())
        case "last":
            if !session.lastText.isEmpty { reply(["ok": true, "text": session.lastText]) }
            else {
                history.read { result in
                    switch result {
                    case .success(let entries): reply(["ok": true, "text": entries.first?.text ?? ""])
                    case .failure(let error): reply(self.failure(error.localizedDescription))
                    }
                }
            }
        case "history":
            if request["show"] as? Bool == true { showHistory(); reply(["ok": true]); return }
            let before: UUID?
            if let value = request["before"] as? String {
                guard let id = UUID(uuidString: value) else { reply(failure("Invalid history cursor")); return }
                before = id
            } else { before = nil }
            history.read(before: before) { result in
                switch result {
                case .success(let entries):
                    var response: [String: Any] = ["ok": true, "entries": entries.map(\.json)]
                    if entries.count == 50, let last = entries.last { response["next_before"] = last.id.uuidString }
                    reply(response)
                case .failure(let error): reply(self.failure(error.localizedDescription))
                }
            }
        case "devices": reply(["ok": true, "devices": AudioDevices.list().map(\.json)])
        case "input":
            guard session.phase == .idle || session.phase == .failed else { reply(failure("Stop recording before changing input")); return }
            guard let uid = request["uid"] as? String, uid == "default" || AudioDevices.list().contains(where: { $0.uid == uid }) else { reply(failure("Unknown microphone UID; run hey-talk devices")); return }
            if uid == "default" { UserDefaults.standard.removeObject(forKey: "inputUID") }
            else { UserDefaults.standard.set(uid, forKey: "inputUID") }
            reply(status())
        case "overlay": showOverlay(request["visible"] as? Bool ?? true); reply(status())
        case "warm":
            inference.warm { result in DispatchQueue.main.async {
                switch result { case .success: reply(self.status()); self.scheduleRelease()
                case .failure(let error): reply(self.failure(error.localizedDescription)) }
            } }
        case "transcribe":
            guard let file = request["file"] as? String else { reply(failure("Missing audio file")); return }
            do {
                let id = try session.begin(); try session.processing(id)
                paste = false; transcriptionSource = "file"; error = nil; pendingReply = reply
                idleRelease?.cancel(); overlay?.update(.processing)
                DispatchQueue.global(qos: .userInitiated).async {
                    do {
                        let samples = try AudioCapture.readFile(file)
                        DispatchQueue.main.async {
                            guard self.session.generation == id else { return }
                            self.decode(samples, id: id)
                        }
                    } catch { DispatchQueue.main.async { if self.session.generation == id { self.fail(error.localizedDescription) } } }
                }
            } catch { reply(failure(error.localizedDescription)) }
        case "permissions":
            if request["kind"] as? String == "microphone" {
                AVCaptureDevice.requestAccess(for: .audio) { _ in DispatchQueue.main.async { reply(self.status()) } }
            } else if request["kind"] as? String == "accessibility" {
                let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
                _ = AXIsProcessTrustedWithOptions(options); reply(status())
            } else { reply(failure("Use permissions microphone or permissions accessibility")) }
        case "quit":
            reply(["ok": true, "state": "stopped"])
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { NSApp.terminate(nil) }
        default: reply(failure("Unknown command: \(command)"))
        }
    }
    private func start(showOverlay: Bool, paste: Bool, reply: @escaping ([String: Any]) -> Void) {
        let id: UInt64
        do { id = try session.begin() } catch { reply(failure(error.localizedDescription)); return }
        pendingStartReply = reply
        error = nil; captureReady = false; self.paste = paste; transcriptionSource = "microphone"
        target = NSWorkspace.shared.frontmostApplication
        idleRelease?.cancel(); self.showOverlay(showOverlay)
        overlay?.preparing(); hotkey?.captureEscape(true)
        let begin: () -> Void = {
            guard self.session.generation == id, self.session.phase == .recording else { return }
            let start = ProcessInfo.processInfo.systemUptime
            self.inference.warm()
            self.capture.start(uid: self.inputUID) { result in
                guard self.session.generation == id, self.session.phase == .recording else { return }
                switch result {
                case .failure(let error): self.fail(error.localizedDescription)
                case .success:
                self.captureMilliseconds = (ProcessInfo.processInfo.systemUptime - start) * 1000
                self.captureReady = true
                self.overlay?.update(.recording); self.hotkey?.captureEscape(true)
                self.cues.start()
                let limit = DispatchWorkItem { [weak self] in
                    guard let self, self.session.generation == id, self.session.phase == .recording else { return }
                    self.stop { _ in }
                }
                self.maximumDuration = limit
                DispatchQueue.main.asyncAfter(deadline: .now() + 299, execute: limit)
                self.pendingStartReply?(self.status()); self.pendingStartReply = nil
                }
            }
        }
        if AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
            AVCaptureDevice.requestAccess(for: .audio) { _ in DispatchQueue.main.async(execute: begin) }
        } else { begin() }
    }
    private func stop(_ reply: @escaping ([String: Any]) -> Void) {
        guard session.phase == .recording else { reply(failure("No recording is active")); return }
        maximumDuration?.cancel(); maximumDuration = nil
        let id = session.generation
        guard captureReady else { cancel(); reply(failure("Released before the microphone was ready")); return }
        captureReady = false
        do { try session.processing(id) } catch { reply(failure(error.localizedDescription)); return }
        overlay?.update(.processing); pendingReply = reply
        capture.stop { result in
            guard self.session.generation == id else { return }
            switch result {
            case .failure(let error): self.fail(error.localizedDescription)
            case .success(let samples):
                self.cues.stop()
                guard samples.count >= 2400 else { self.cancel(); return }
                self.decode(samples, id: id)
            }
        }
    }
    private func decode(_ samples: [Float], id: UInt64) {
        let audioSeconds = Double(samples.count) / 16_000
        inference.transcribe(samples) { result in DispatchQueue.main.async {
            guard self.session.generation == id else { return }
            switch result {
            case .failure(let error): self.fail(error.localizedDescription)
            case .success(let (text, elapsed)):
                guard !text.isEmpty else { self.fail("No speech detected. Check your microphone with hey-talk devices."); return }
                guard self.session.finish(id, text: text) else { return }
                self.hotkey?.reset(); self.overlay?.update(.idle)
                let entry = Dictation(text: text, audioSeconds: audioSeconds, source: self.transcriptionSource)
                // Detach this result before disk IO: a new recording may start while saving.
                let reply = self.pendingReply; self.pendingReply = nil
                let shouldPaste = self.paste, target = self.target, captureMS = self.captureMilliseconds
                self.history.save(entry) { saved in
                    let complete: (String) -> Void = { delivery in
                        reply?(["ok": true, "text": text, "audio_seconds": audioSeconds,
                                "inference_ms": elapsed * 1000, "real_time_factor": elapsed / audioSeconds,
                                "capture_start_ms": captureMS, "delivery": delivery,
                                "history_id": entry.id.uuidString, "history_saved": saved])
                        if self.session.generation == id {
                            self.scheduleRelease()
                            if !saved { self.overlay?.error("History could not be saved. Copy your text from Dictation History before quitting.") }
                            else if delivery == "clipboard" { self.overlay?.error("Copied to clipboard and saved in Dictation History. Press ⌘V to paste.") }
                        }
                    }
                    if shouldPaste, self.session.generation == id {
                        Paste.deliver(text, target: target, completion: complete)
                    } else { complete(shouldPaste ? "history" : "stdout") }
                }
            }
        } }
    }
    private func cancel() {
        captureReady = false
        session.cancel(); maximumDuration?.cancel(); maximumDuration = nil
        capture.cancel(); inference.cancel(); hotkey?.reset(); overlay?.update(.idle)
        pendingStartReply?(failure("Cancelled")); pendingStartReply = nil
        pendingReply?(failure("Cancelled")); pendingReply = nil
    }
    private func fail(_ message: String) {
        captureReady = false
        capture.cancel(); session.fail(session.generation); hotkey?.reset()
        maximumDuration?.cancel(); maximumDuration = nil
        error = message; overlay?.update(.failed); overlay?.error(message)
        pendingStartReply?(failure(message)); pendingStartReply = nil
        pendingReply?(failure(message)); pendingReply = nil; scheduleRelease()
    }
    private func failure(_ message: String) -> [String: Any] { ["ok": false, "error": message] }
    private func scheduleRelease() {
        idleRelease?.cancel()
        let item = DispatchWorkItem { [weak self] in
            guard let self, self.session.phase == .idle || self.session.phase == .failed else { return }
            self.inference.cancel()
        }
        idleRelease = item
        DispatchQueue.main.asyncAfter(deadline: .now() + 600, execute: item)
    }
}
