import AppKit
import AVFoundation
import TalkCore

final class TalkService: NSObject, NSApplicationDelegate {
    private let server = CommandServer()
    private let capture = AudioCapture()
    private let inference = Inference()
    private let correction = Correction()
    private let correctionSettings = CorrectionSettings()
    private var correctionMenuItem: NSMenuItem?
    private var overlayMenuItem: NSMenuItem?
    private var speechMenuItem: NSMenuItem?
    private let speechOffBadge = CALayer()
    private let speech = SpeechController()
    private var speechWasActive = false
    private let history = DictationHistory()
    private let enterExceptions = EnterExceptions()
    private let vocabulary = SharedVocabulary()
    private let vocabularyQueue = DispatchQueue(label: "hey-talk.vocabulary", qos: .utility)
    private var vocabularyTimer: DispatchSourceTimer?
    private var vocabularyRefreshPending = false
    private var vocabularySync: [String: Any] = ["source": "pending"]
    private let autoSettings = AutoDictationSettings()
    private let focusMonitor = InputFocusMonitor()
    private let autoSubmitKey = AutoSubmitKey()
    private var focusTrigger = FocusTrigger<FocusedInput>()
    private var automatic: AutoRecording<FocusedInput>?
    private var autoTarget: FocusedInput? { automatic?.target }
    private var autoDeliveryValid: Bool { automatic?.deliveryValid ?? false }
    private var focusGrace: DispatchWorkItem?
    private var autoRestart: DispatchWorkItem?
    private var autoMenuItem: NSMenuItem?
    private var historyWindow: HistoryWindow?
    private lazy var accessibilitySetup = AccessibilitySetup()
    private var session = RecordingSession()
    private var overlay: Overlay?
    private var hotkey: Hotkey?
    private var menuItem: NSStatusItem?
    private var pendingReply: (([String: Any]) -> Void)?
    private var pendingStartReply: (([String: Any]) -> Void)?
    private var target: NSRunningApplication?
    private var targetName: String?
    private var paste = true
    private var pressEnter = false
    private var transcriptionSource = "microphone"
    private var overlayEnabled: Bool
    private var error: String?
    private var lastPasteBlock: PasteBlockReason?
    private var captureReady = false
    private var captureMilliseconds: Double = 0
    private var idleRelease: DispatchWorkItem?
    private var maximumDuration: DispatchWorkItem?
    private var signals: [DispatchSourceSignal] = []
    private var inputUID: String? { UserDefaults.standard.string(forKey: "inputUID") }

    init(headless: Bool) { overlayEnabled = !headless; super.init() }
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        NativeMenus.install()
        do { try server.start() }
        catch { fputs("hey-talk: \(error.localizedDescription)\n", stderr); NSApp.terminate(nil); return }
        server.handle = { [weak self] request, reply in self?.command(request, reply: reply) }
        capture.onFailure = { [weak self] message in self?.fail(message) }
        capture.onReadinessChanged = { [weak self] ready in
            guard let self, self.session.phase == .recording else { return }
            if ready { self.updateRecordingOverlay() } else { self.overlay?.preparing() }
        }
        hotkey = Hotkey()
        hotkey?.onAction = { [weak self] action in
            guard let self else { return }
            switch action {
            case .toggle(let shortcut): self.toggleDictation(shortcut: shortcut)
            case .toggleAuto: self.toggleAutoMode()
            case .toggleSpeech: self.speech.toggle()
            case .toggleSpeechMode: self.speech.toggleEnabled()
            case .cancel:
                if self.speech.active { self.speech.stop() }
                else if self.session.phase == .recording || self.session.phase == .processing || self.autoTarget != nil || self.autoRestart != nil { self.cancel() }
            case .none: break
            }
        }
        speech.canOffer = { [weak self] in
            guard let self else { return false }
            return self.session.phase != .processing && (self.session.phase != .recording || self.autoTarget != nil)
        }
        speech.willPlay = { [weak self] in
            guard let self else { return false }
            guard self.session.phase != .processing && (self.session.phase != .recording || self.autoTarget != nil) else {
                self.overlay?.error("Finish dictating before reading aloud."); return false
            }
            if self.autoTarget != nil || self.autoRestart != nil { self.cancel() }
            return true
        }
        speech.onAllowedChanged = { [weak self] allowed in
            guard let self else { return }
            self.hotkey?.enableSpeechMode(allowed)
            self.hotkey?.enableSpeech(allowed && self.speech.settings.enabled)
        }
        speech.onNotice = { [weak self] message in self?.showOverlay(true); self?.overlay?.error(message) }
        speech.onSettingsChanged = { [weak self] in self?.applySpeechMode() }
        speech.onActivityChanged = { [weak self] active in
            guard let self else { return }
            self.hotkey?.captureEscape(active || self.session.phase == .recording || self.session.phase == .processing)
            let ended = self.speechWasActive && !active
            self.speechWasActive = active
            if ended {
                self.focusTrigger.reset()
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [weak self] in
                    guard let self, !self.speech.active else { return }
                    self.focusedInputChanged(FocusedInput.current(bundleIDs: self.autoSettings.bundleIDs))
                }
            }
        }
        speech.start()
        menuItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        menuItem?.button?.image = NSImage(systemSymbolName: "waveform", accessibilityDescription: "Hey Talk")
        if let button = menuItem?.button {
            button.wantsLayer = true
            speechOffBadge.contents = TalkDesign.speechOffImage?.cgImage(forProposedRect: nil, context: nil, hints: nil)
            speechOffBadge.contentsGravity = .resizeAspect
            speechOffBadge.frame = CGRect(x: button.bounds.maxX - 11, y: 1, width: 10, height: 9)
            button.layer?.addSublayer(speechOffBadge)
        }
        let menu = NSMenu()
        for (title, action) in [("Dictation History…", #selector(showHistory)), ("Automatic Paste…", #selector(showAccessibilitySetup)), ("Start / Stop Dictation", #selector(toggle)), ("Cancel Dictation", #selector(cancelMenu)), ("Hide Pill", #selector(toggleOverlay)), ("Quit Hey Talk", #selector(quit))] {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: ""); item.target = self; menu.addItem(item)
            if action == #selector(toggleOverlay) { overlayMenuItem = item }
        }
        menuItem?.menu = menu
        let autoItem = NSMenuItem(title: "Auto Mode · Hyper+V", action: #selector(toggleAutoMode), keyEquivalent: "")
        autoItem.target = self; menu.insertItem(autoItem, at: 2); autoMenuItem = autoItem
        let correctionItem = NSMenuItem(title: "AI Correction", action: #selector(toggleCorrection), keyEquivalent: "")
        correctionItem.target = self; menu.insertItem(correctionItem, at: 3); correctionMenuItem = correctionItem
        let speechItem = NSMenuItem(title: "Read Aloud · Hyper+T", action: #selector(toggleSpeechMode), keyEquivalent: "")
        speechItem.target = self; menu.insertItem(speechItem, at: 4); speechMenuItem = speechItem
        applySpeechMode()
        applyCorrection()
        focusMonitor.onChange = { [weak self] input in self?.focusedInputChanged(input) }
        focusMonitor.onSuspend = { [weak self] in
            guard let self, self.autoTarget != nil || self.autoRestart != nil else { return }
            self.cancel()
        }
        autoSubmitKey.shouldCapture = { [weak self] in
            guard let self, let target = self.autoTarget else { return false }
            if self.session.phase == .recording {
                let input = FocusedInput.current(bundleIDs: self.autoSettings.bundleIDs)
                if input != target { self.autoSubmitKey.passNextEnter = false }
                return input != nil
            }
            if self.session.phase == .idle && !self.autoSubmitKey.passNextEnter { return false }
            return self.autoDeliveryValid && target.isFocused()
        }
        autoSubmitKey.onSubmit = { [weak self] in
            guard let self, self.session.phase == .recording else { return }
            self.pressEnter = true; self.stop { _ in }
        }
        autoSubmitKey.onInterruption = { [weak self] in
            guard let self, self.autoTarget != nil else { return }
            self.cancel(); self.overlay?.error("Auto paused. Focus the message field again.")
        }
        history.refresh()
        let vocabularyTimer = DispatchSource.makeTimerSource(queue: .main)
        vocabularyTimer.schedule(deadline: .now(), repeating: .seconds(15), leeway: .seconds(3))
        vocabularyTimer.setEventHandler { [weak self] in self?.refreshVocabulary() }
        vocabularyTimer.resume(); self.vocabularyTimer = vocabularyTimer
        showOverlay(overlayEnabled)
        applyAutoMode()
        if CommandLine.arguments.contains("--show-history") { showHistory() }
        for number in [SIGTERM, SIGINT] {
            signal(number, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: number, queue: .main)
            source.setEventHandler { NSApp.terminate(nil) }; source.resume(); signals.append(source)
        }
        NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main) { [weak self] _ in self?.cancel(keepMicrophoneWarm: false) }
    }
    func applicationWillTerminate(_ notification: Notification) { speech.shutdown(); vocabularyTimer?.cancel(); focusMonitor.stop(); autoSubmitKey.stop(); capture.cancel(); inference.cancel(); correction.cancel(); history.finishPendingWrites() }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        showOverlay(true); showHistory(); return true
    }
    @objc private func showAccessibilitySetup() { accessibilitySetup.show() }
    @objc private func showHistory() {
        if historyWindow == nil { historyWindow = HistoryWindow(history: history) }
        history.refresh(); historyWindow?.show()
    }
    private func showOverlay(_ show: Bool) {
        if overlay == nil {
            let view = Overlay()
            view.onToggle = { [weak self] in self?.toggle() }
            view.onToggleSpeech = { [weak self] in self?.speech.toggleEnabled() }
            view.onToggleAuto = { [weak self] in self?.toggleAutoMode() }
            view.onToggleCorrection = { [weak self] in self?.toggleCorrection() }
            view.setSpeechEnabled(speech.settings.enabled)
            view.onOpenAccessibility = { [weak self] in
                self?.accessibilitySetup.show()
                self?.accessibilitySetup.openSettings()
            }
            view.level = { [weak self] in self?.capture.level ?? 0 }
            view.setAutoMode(autoSettings.enabled)
            view.setCorrection(correctionSettings.enabled)
            overlay = view
        }
        overlayEnabled = show; overlay?.show(show)
        overlayMenuItem?.title = show ? "Hide Pill" : "Show Pill"
        updateStatusTooltip()
    }
    @objc private func toggleOverlay() { showOverlay(!overlayEnabled) }
    @objc private func toggleSpeechMode() { speech.toggleEnabled() }
    private func applySpeechMode() {
        let enabled = speech.settings.enabled
        speechMenuItem?.state = enabled ? .on : .off
        speechMenuItem?.image = enabled ? NSImage(systemSymbolName: "speaker.wave.2.fill", accessibilityDescription: "Read aloud on") : TalkDesign.speechOffImage
        speechOffBadge.isHidden = enabled
        overlay?.setSpeechEnabled(enabled)
        updateStatusTooltip()
    }
    private func updateStatusTooltip() {
        let text = "Hey Talk is running · Pill \(overlayEnabled ? "visible" : "hidden") · Read aloud \(speech.settings.enabled ? "on" : "off")"
        menuItem?.button?.toolTip = text
        menuItem?.button?.setAccessibilityLabel(text)
    }
    @objc private func toggleCorrection() {
        try? correctionSettings.command(["toggle"])
        applyCorrection()
    }
    private func applyCorrection() {
        correctionMenuItem?.state = correctionSettings.enabled ? .on : .off
        overlay?.setCorrection(correctionSettings.enabled)
    }
    @objc private func toggleAutoMode() {
        autoSettings.enabled.toggle()
        showOverlay(true)
        applyAutoMode()
        if !autoSettings.enabled || autoSubmitKey.ready { overlay?.showModeNotice() }
    }
    private func applyAutoMode() {
        if autoTarget != nil || autoRestart != nil { cancel() }
        focusMonitor.stop(); focusTrigger.reset()
        autoMenuItem?.state = autoSettings.enabled ? .on : .off
        overlay?.setAutoMode(autoSettings.enabled)
        if autoSettings.enabled {
            if !autoSubmitKey.start() {
                overlay?.error("Auto needs Accessibility. Enable automatic paste.", accessibilityAction: true)
            }
            focusMonitor.start(bundleIDs: autoSettings.bundleIDs)
        } else { autoSubmitKey.stop() }
    }
    private func focusedInputChanged(_ input: FocusedInput?, notice: String? = nil) {
        if input != autoTarget { autoSubmitKey.passNextEnter = false }
        if automatic != nil {
            automatic?.focusChanged(input, at: ProcessInfo.processInfo.systemUptime)
            if session.phase == .recording {
                if let input {
                    focusGrace?.cancel(); focusGrace = nil
                    target = input.app; targetName = input.app.localizedName
                } else if focusGrace == nil {
                    let id = session.generation
                    let timeout = DispatchWorkItem { [weak self] in
                        guard let self, self.session.generation == id, self.session.phase == .recording else { return }
                        self.focusMonitor.refresh()
                        if self.automatic?.shouldCancel(at: ProcessInfo.processInfo.systemUptime) == true { self.cancel() }
                    }
                    focusGrace = timeout
                    DispatchQueue.main.asyncAfter(deadline: .now() + AutoRecording<FocusedInput>.focusGrace, execute: timeout)
                }
            }
        }
        let available = !speech.active && autoSettings.enabled && autoTarget == nil && autoRestart == nil && (session.phase == .idle || session.phase == .failed)
        guard focusTrigger.update(input, available: available), let input, autoSubmitKey.start() else { return }
        start(showOverlay: true, paste: true, focusedInput: input) { [weak self] result in
            if result["ok"] as? Bool == true, let notice { self?.overlay?.error(notice) }
        }
    }
    private func restartAutomatic(id: UInt64, notice: String? = nil) {
        let restart = DispatchWorkItem { [weak self] in
            guard let self, self.session.generation == id else { return }
            self.autoRestart = nil; self.hotkey?.reset()
            guard self.autoSettings.enabled, self.session.phase == .idle,
                  let input = FocusedInput.current(bundleIDs: self.autoSettings.bundleIDs) else { self.clearAutomatic(); return }
            // Carry an unused pass-through into this same input's next recording.
            let passNextEnter = self.autoSubmitKey.passNextEnter && self.autoTarget == input
            self.automatic = nil
            self.focusTrigger.reset()
            self.focusedInputChanged(input, notice: notice)
            if self.autoTarget == input { self.autoSubmitKey.passNextEnter = passNextEnter }
        }
        autoRestart = restart
        hotkey?.captureEscape(true)
        // Give the app time to consume Return and clear or replace its editor.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25, execute: restart)
    }
    @objc private func toggle() {
        toggleDictation(shortcut: nil)
    }
    private func toggleDictation(shortcut: DictationShortcut?) {
        switch DictationToggle.resolve(phase: session.phase, currentSubmit: pressEnter, requestedSubmit: shortcut.map { $0 == .submit }) {
        case .stop(let submit): pressEnter = submit; stop { _ in }
        case .start(let submit): start(showOverlay: overlayEnabled, paste: true, pressEnter: submit) { _ in }
        case .busy: break
        }
    }
    @objc private func cancelMenu() { cancel() }
    @objc private func quit() { NSApp.terminate(nil) }

    private func status() -> [String: Any] {
        var value: [String: Any] = [
            "ok": true, "state": session.phase.rawValue, "pid": getpid(),
            "model": "orukeet-v0.1.0", "model_ready": ModelPaths.available,
            "hotkey_mode": "toggle", "hotkey": DictationShortcut.submit.name, "hotkey_registered": hotkey?.registered ?? false,
            "hotkeys": ["paste": DictationShortcut.paste.name, "submit": DictationShortcut.submit.name, "auto": "Control+Command+Option+Shift+V"],
            "enter_exceptions": enterExceptions.bundleIDs,
            "auto": autoSettings.json, "auto_ready": autoSubmitKey.ready,
            "correction": correctionSettings.json,
            "auto_recording": autoTarget != nil, "auto_focused": focusMonitor.current != nil,
            "escape_registered": hotkey?.escapeRegistered ?? false, "hotkey_events": hotkey?.eventsReceived ?? 0,
            "overlay": overlayEnabled, "input": inputUID ?? "default",
            "sample_count": capture.sampleCount, "rms": capture.level,
            "capture_start_ms": captureMilliseconds, "peak_rms": capture.maximumLevel,
            "microphone_ready": captureReady && capture.isReady,
            "microphone_keepalive": capture.isKeepingAlive,
            "capture_restarts": capture.restartCount,
            "bluetooth_input": capture.usesBluetooth,
            "microphone": permissionName, "accessibility": AXIsProcessTrusted(),
        ]
        value["vocabulary_sync"] = vocabularySync
        value["speech"] = speech.status
        value["speech_hotkey_registered"] = hotkey?.speechRegistered ?? false
        value["speech_mode_hotkey_registered"] = hotkey?.speechModeRegistered ?? false
        if let input = capture.activeInput { value["active_input"] = input.json }
        if let input = capture.fallbackSource { value["fallback_from"] = input.json }
        if let pid = inference.pid { value["inference_pid"] = pid }
        if let error { value["last_error"] = error }
        if let reason = lastPasteBlock { value["last_paste_block"] = reason.rawValue }
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
    private func refreshVocabulary(_ arguments: [String] = [], reply: (([String: Any]) -> Void)? = nil) {
        let background = reply == nil
        if background {
            guard !vocabularyRefreshPending else { return }
            vocabularyRefreshPending = true
        }
        vocabularyQueue.async { [weak self] in
            guard let self else { return }
            let result: [String: Any]
            do { result = try self.vocabulary.command(arguments) }
            catch { result = ["ok": false, "error": error.localizedDescription] }
            DispatchQueue.main.async {
                if background { self.vocabularyRefreshPending = false }
                self.vocabularySync = ["source": result["source"] ?? "cache"]
                if let error = result["sync_error"] ?? result["error"] { self.vocabularySync["error"] = error }
                reply?(result)
            }
        }
    }
    private func command(_ request: [String: Any], reply: @escaping ([String: Any]) -> Void) {
        let command = request["command"] as? String ?? ""
        switch command {
        case "speech":
            do {
                guard let args = request["arguments"] as? [String] else { throw TalkError("Missing speech arguments") }
                try SpeechSettings.validate(args)
                switch args.first {
                case "target":
                    speech.target { result in
                        switch result {
                        case .success(let target): var value = target.json; value["ok"] = true; reply(value)
                        case .failure(let error): reply(self.failure(error.localizedDescription))
                        }
                    }
                    return
                case "play":
                    speech.toggle { result in
                        switch result {
                        case .success: reply(["ok": true, "speech": self.speech.status])
                        case .failure(let error): reply(self.failure(error.localizedDescription))
                        }
                    }
                    return
                case "pause": speech.pause()
                case "stop": speech.stop()
                default:
                    let wasEnabled = speech.settings.enabled, apps = speech.settings.bundleIDs
                    try speech.settings.command(args)
                    if wasEnabled != speech.settings.enabled || apps != speech.settings.bundleIDs { speech.settingsChanged() }
                }
                reply(["ok": true, "speech": speech.status])
            } catch { reply(failure(error.localizedDescription)) }
        case "status", "doctor": reply(status())
        case "correction":
            do {
                guard let arguments = request["arguments"] as? [String] else { throw TalkError("Missing correction arguments") }
                try correctionSettings.command(arguments)
                applyCorrection()
                reply(["ok": true, "correction": correctionSettings.json])
            } catch { reply(failure(error.localizedDescription)) }
        case "auto":
            do {
                guard let arguments = request["arguments"] as? [String] else { throw TalkError("Missing Auto arguments") }
                let wasEnabled = autoSettings.enabled, apps = autoSettings.bundleIDs
                try autoSettings.command(arguments)
                if wasEnabled != autoSettings.enabled || apps != autoSettings.bundleIDs {
                    showOverlay(true); applyAutoMode()
                    if !autoSettings.enabled || autoSubmitKey.ready { overlay?.showModeNotice() }
                }
                reply(["ok": true, "auto": autoSettings.json, "auto_ready": autoSubmitKey.ready])
            } catch { reply(failure(error.localizedDescription)) }
        case "vocabulary":
            guard let arguments = request["arguments"] as? [String] else { reply(failure("Missing vocabulary arguments")); return }
            refreshVocabulary(arguments, reply: reply)
        case "enter-exceptions":
            do {
                guard let arguments = request["arguments"] as? [String] else { throw TalkError("Missing exception arguments") }
                reply(["ok": true, "enter_exceptions": try enterExceptions.command(arguments)])
            } catch { reply(failure(error.localizedDescription)) }
        case "start", "stop", "toggle":
            let submit = request["submit"] as? Bool
            guard request["submit"] == nil || submit != nil else { reply(failure("submit must be a boolean")); return }
            guard submit != true || (request["paste"] as? Bool != false) else { reply(failure("submit requires paste")); return }
            if command == "start" {
                start(showOverlay: request["overlay"] as? Bool ?? overlayEnabled, paste: request["paste"] as? Bool ?? true, pressEnter: submit ?? false, reply: reply)
            } else if command == "stop" {
                if session.phase == .recording, let submit { pressEnter = submit }
                stop(reply)
            } else {
                switch DictationToggle.resolve(phase: session.phase, currentSubmit: pressEnter, requestedSubmit: submit) {
                case .stop(let submit): pressEnter = submit; stop(reply)
                case .start(let submit): start(showOverlay: request["overlay"] as? Bool ?? overlayEnabled, paste: request["paste"] as? Bool ?? true, pressEnter: submit, reply: reply)
                case .busy: reply(failure("Already processing"))
                }
            }
        case "cancel": cancel(); reply(status())
        case "last":
            if let entry = history.entries.first { reply(entry.json.merging(["ok": true]) { _, new in new }) }
            else {
                history.read { result in
                    switch result {
                    case .success(let entries): reply((entries.first?.json ?? ["text": "", "original_text": NSNull()]).merging(["ok": true]) { _, new in new })
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
            capture.cancel()
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
                clearAutomatic()
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
                showAccessibilitySetup(); reply(status())
            } else { reply(failure("Use permissions microphone or permissions accessibility")) }
        case "quit":
            reply(["ok": true, "state": "stopped"])
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { NSApp.terminate(nil) }
        default: reply(failure("Unknown command: \(command)"))
        }
    }
    private func start(showOverlay: Bool, paste: Bool, pressEnter: Bool = false, focusedInput: FocusedInput? = nil,
                       reply: @escaping ([String: Any]) -> Void) {
        let id: UInt64
        do { id = try session.begin() } catch { reply(failure(error.localizedDescription)); return }
        speech.stop()
        pendingStartReply = reply
        error = nil; captureReady = false; captureMilliseconds = 0
        self.paste = paste; self.pressEnter = pressEnter; transcriptionSource = "microphone"
        clearAutomatic()
        automatic = focusedInput.map { AutoRecording(target: $0) }
        target = focusedInput?.app ?? NSWorkspace.shared.frontmostApplication
        targetName = target?.localizedName
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
                self.updateRecordingOverlay(); self.hotkey?.captureEscape(true)
                let limit = DispatchWorkItem { [weak self] in
                    guard let self, self.session.generation == id, self.session.phase == .recording else { return }
                    if self.autoTarget != nil {
                        self.cancel(); self.overlay?.error("Auto timed out. Focus the message field again.")
                    } else { self.stop { _ in } }
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
    private func updateRecordingOverlay() {
        overlay?.update(.recording)
        if capture.fallbackSource != nil, let input = capture.activeInput {
            overlay?.error("Using \(input.name). Bluetooth microphone unavailable.")
        }
    }
    private func stop(_ reply: @escaping ([String: Any]) -> Void) {
        guard session.phase == .recording else { reply(failure("No recording is active")); return }
        guard captureReady else {
            // An early Enter must not cancel Auto while the microphone opens.
            if autoTarget == nil { cancel() }
            reply(failure("Stopped before the microphone was ready")); return
        }
        if automatic != nil {
            guard let input = FocusedInput.current(bundleIDs: autoSettings.bundleIDs) else {
                reply(failure("Focus an allowed input before finishing Auto dictation")); return
            }
            automatic?.finish(in: input)
            target = input.app; targetName = input.app.localizedName
        }
        autoSubmitKey.passNextEnter = false
        focusGrace?.cancel(); focusGrace = nil
        maximumDuration?.cancel(); maximumDuration = nil
        let id = session.generation
        captureReady = false
        do { try session.processing(id) } catch { reply(failure(error.localizedDescription)); return }
        overlay?.update(.processing); pendingReply = reply
        capture.stop { result in
            guard self.session.generation == id else { return }
            switch result {
            case .failure(let error): self.fail(error.localizedDescription)
            case .success(let samples):
                guard samples.count >= 2400 else {
                    if self.autoTarget != nil { self.noSpeechDetected(id: id) } else { self.cancel() }
                    return
                }
                self.decode(samples, id: id)
            }
        }
    }
    private func decode(_ samples: [Float], id: UInt64) {
        let audioSeconds = Double(samples.count) / 16_000
        let useCorrection = correctionSettings.enabled
        overlay?.processing(audioSeconds: audioSeconds, correction: useCorrection)
        hotkey?.captureEscape(true)
        inference.transcribe(samples) { result in DispatchQueue.main.async {
            guard self.session.generation == id else { return }
            switch result {
            case .failure(let error): self.fail(error.localizedDescription)
            case .success(let (text, elapsed)):
                guard !text.isEmpty else { self.noSpeechDetected(id: id); return }
                guard useCorrection else {
                    self.finish(text, originalText: text, id: id, audioSeconds: audioSeconds, elapsed: elapsed, correctionInfo: ["status": "off"])
                    return
                }
                let started = ProcessInfo.processInfo.systemUptime
                self.correction.correct(text, vocabulary: (try? Vocabulary().words()) ?? []) { result in
                    guard self.session.generation == id, self.session.phase == .processing else { return }
                    var info: [String: Any] = ["elapsed_ms": (ProcessInfo.processInfo.systemUptime - started) * 1000]
                    let corrected: String
                    switch result {
                    case .success(let value): corrected = value; info["status"] = "applied"
                    case .failure(let error):
                        corrected = text; info["status"] = "fallback"; info["error"] = error.localizedDescription
                    }
                    self.finish(corrected, originalText: text, id: id, audioSeconds: audioSeconds, elapsed: elapsed, correctionInfo: info)
                }
            }
        } }
    }
    private func finish(_ text: String, originalText: String, id: UInt64, audioSeconds: Double, elapsed: Double, correctionInfo: [String: Any]) {
        guard self.session.finish(id, text: text) else { return }
        if self.autoTarget == nil { self.hotkey?.reset() }
        self.overlay?.finishProcessing()
        let entry = Dictation(text: text, originalText: originalText, audioSeconds: audioSeconds, source: self.transcriptionSource)
        // Detach this result before disk IO: a new recording may start while saving.
        let reply = self.pendingReply; self.pendingReply = nil
        let shouldPaste = self.paste, pressEnter = self.pressEnter, target = self.target, captureMS = self.captureMilliseconds
        let targetName = self.targetName
        let focusedInput = self.autoTarget
        self.history.save(entry) { saved in
            let complete: (String, PasteBlockReason?) -> Void = { delivery, reason in
                self.lastPasteBlock = reason
                var response: [String: Any] = ["ok": true, "text": text, "original_text": originalText, "audio_seconds": audioSeconds,
                        "inference_ms": elapsed * 1000, "real_time_factor": elapsed / audioSeconds,
                        "capture_start_ms": captureMS, "delivery": delivery,
                        "history_id": entry.id.uuidString, "history_saved": saved, "correction": correctionInfo]
                if let reason { response["paste_block"] = reason.rawValue }
                reply?(response)
                if self.session.generation == id {
                    self.automatic = nil; self.hotkey?.reset()
                    self.scheduleRelease()
                    if !saved { self.overlay?.error("Not saved. Copy from History before quitting.") }
                    else if let reason {
                        let message = reason.message(targetName: targetName, pasted: delivery == "pasted")
                        self.overlay?.error(message, accessibilityAction: reason == .accessibility)
                    }
                    if correctionInfo["status"] as? String == "fallback", saved, self.lastPasteBlock == nil { self.overlay?.error("AI unavailable. Original transcript kept.") }
                    if saved, delivery == "submitted", focusedInput != nil { self.restartAutomatic(id: id) }
                }
            }
            if shouldPaste, self.session.generation == id {
                let submit = self.enterExceptions.shouldPressEnter(requested: pressEnter, bundleID: target?.bundleIdentifier)
                Paste.deliver(text, target: target, pressEnter: submit,
                              targetStillFocused: { focusedInput.map { self.autoDeliveryValid && $0.isFocused() } ?? true },
                              isCurrent: { self.session.generation == id }, completion: complete)
            } else { complete(shouldPaste ? "history" : "stdout", nil) }
        }
    }
    private func noSpeechDetected(id: UInt64) {
        let message = "No speech detected. Check your microphone with hey-talk devices."
        guard autoTarget != nil else { fail(message, keepMicrophoneWarm: true); return }
        guard session.finish(id, text: "") else { return }
        // Keep the destination through the restart delay so a second Enter can
        // pass through immediately, even before the microphone reopens.
        autoSubmitKey.passNextEnter = autoDeliveryValid
        hotkey?.reset()
        overlay?.update(.idle)
        let notice = OverlayNotice.concise(message)
        overlay?.error(notice)
        pendingReply?(failure(message)); pendingReply = nil
        scheduleRelease()
        restartAutomatic(id: id, notice: notice)
    }
    private func cancel(keepMicrophoneWarm: Bool = true) {
        captureReady = false
        clearAutomatic()
        session.cancel(); maximumDuration?.cancel(); maximumDuration = nil
        capture.cancel(keepWarm: keepMicrophoneWarm); inference.cancel(); correction.cancel(); hotkey?.reset(); overlay?.update(.idle)
        pendingStartReply?(failure("Cancelled")); pendingStartReply = nil
        pendingReply?(failure("Cancelled")); pendingReply = nil
    }
    private func fail(_ message: String, keepMicrophoneWarm: Bool = false) {
        captureReady = false
        clearAutomatic()
        capture.cancel(keepWarm: keepMicrophoneWarm); session.fail(session.generation); hotkey?.reset()
        correction.cancel()
        maximumDuration?.cancel(); maximumDuration = nil
        error = message; overlay?.update(.failed); overlay?.error(OverlayNotice.concise(message))
        pendingStartReply?(failure(message)); pendingStartReply = nil
        pendingReply?(failure(message)); pendingReply = nil; scheduleRelease()
    }
    private func clearAutomatic() {
        automatic = nil
        autoSubmitKey.passNextEnter = false
        focusGrace?.cancel(); focusGrace = nil
        autoRestart?.cancel(); autoRestart = nil
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
