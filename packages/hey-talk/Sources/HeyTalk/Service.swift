import AppKit
import AVFoundation
import TalkCore

final class TalkService: NSObject, NSApplicationDelegate {
    private let server = CommandServer()
    private let capture: AudioCapture
    private let chatRequest: ([String: Any]) throws -> [String: Any]?
    private let inference = Inference()
    private let commands = RecentCommands()
    private var previousCommands: [[String]] = []
    private let correction = Correction()
    private let contextQueue = DispatchQueue(label: "hey-talk.app-context", qos: .userInitiated)
    private let correctionSettings = CorrectionSettings()
    private var returnToField: Bool { UserDefaults.standard.bool(forKey: "returnToField") }
    private var returnMenuItem: NSMenuItem?
    private var correctionMenuItem: NSMenuItem?
    private var overlayMenuItem: NSMenuItem?
    private let selectionCapture = SelectionCapture()
    private let speech = SpeechController()
    private var speechWasActive = false
    private let history: DictationHistory
    private let enterExceptions = EnterExceptions()
    private lazy var vocabularyWindow = VocabularyWindow()
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
    private lazy var historyWindow = HistoryWindow(history: history)
    private lazy var accessibilitySetup = AccessibilitySetup()
    private var session = RecordingSession() { didSet {
        updateSubmitIndicator()
        if session.phase == .idle || session.phase == .failed { refreshConversation() }
    } }
    private var overlay: Overlay?
    private var conversation: [String: Any]?
    private var conversationFocus: ConversationFocus?
    private var conversationLevel = 0.0
    private var conversationRevision = 0
    private var openingConversation = false
    private var switchingToDictation = false
    private var quitAfterHandoff = false
    private var pausedConversation: String?
    private var resumedDuringProcessing: UInt64?
    private var hotkey: Hotkey?
    private var menuItem: NSStatusItem?
    private var pendingReply: (([String: Any]) -> Void)?
    private var pendingStartReply: (([String: Any]) -> Void)?
    private var target: NSRunningApplication? { didSet { updateSubmitIndicator() } }
    private var slackTeam: String?
    private var recordingHost: String?
    private var targetName: String?
    private typealias ContextSnapshot = Task<(text: String?, milliseconds: Double), Never>
    private var recordingContext: ContextSnapshot?
    private var startingInput: FocusedInput?
    private var waitingPaste: (timer: Timer, text: String)? { didSet { oldValue?.timer.invalidate() } }
    private var paste = true
    private var lastDeckSnapshot = -Double.infinity
    private var autoSubmitEnabled = true
    private var pressEnter = false { didSet { updateSubmitIndicator() } }
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

    init(headless: Bool, capture: AudioCapture = AudioCapture(), history: DictationHistory = DictationHistory(),
         chatRequest: @escaping ([String: Any]) throws -> [String: Any]? = TalkService.requestDesktop) {
        self.capture = capture; self.history = history; self.chatRequest = chatRequest
        overlayEnabled = !headless; super.init()
    }
    func applicationDidFinishLaunching(_ notification: Notification) {
        UserDefaults.standard.register(defaults: ["returnToField": true])
        NSApp.setActivationPolicy(.accessory)
        NativeMenus.install()
        do { try server.start() }
        catch { fputs("hey-talk: \(error.localizedDescription)\n", stderr); NSApp.terminate(nil); return }
        server.deckSnapshot = { [weak self] in self?.deckSnapshot() ?? [:] }
        server.handle = { [weak self] request, reply in self?.command(request, reply: reply) }
        DistributedNotificationCenter.default().addObserver(self, selector: #selector(conversationChanged(_:)), name: .init("local.hey-boss.conversation"), object: "local.hey-boss.desktop")
        for name in [NSWorkspace.didLaunchApplicationNotification, NSWorkspace.didTerminateApplicationNotification] {
            NSWorkspace.shared.notificationCenter.addObserver(self, selector: #selector(conversationAppChanged(_:)), name: name, object: nil)
        }
        NSWorkspace.shared.notificationCenter.addObserver(self, selector: #selector(updateSubmitIndicator),
            name: NSWorkspace.didActivateApplicationNotification, object: nil)
        // Switching tabs doesn't activate a new application.
        let siteTimer = Timer(timeInterval: 1, repeats: true) { [weak self] _ in
            guard let self, self.overlayEnabled || ProcessInfo.processInfo.systemUptime - self.lastDeckSnapshot < 2, BrowserPage.isBrowser(NSWorkspace.shared.frontmostApplication) else { return }
            self.updateSubmitIndicator()
        }
        siteTimer.tolerance = 0.2
        RunLoop.main.add(siteTimer, forMode: .common)
        capture.onFailure = { [weak self] message in self?.fail(message) }
        capture.onSpeechBegan = { [weak self] in
            guard let self, self.session.phase == .listening else { return }
            let input = FocusedInput.current(bundleIDs: self.autoSettings.bundleIDs)
            guard input == self.autoTarget else { self.focusedInputChanged(input); return }
            guard self.session.speechBegan(self.session.generation) else { return }
            self.captureReady = true
            self.recordingBegan()
        }
        capture.onSpeechEnded = { [weak self] in
            guard let self, self.session.phase == .recording, self.autoTarget != nil else { return }
            self.stop { _ in }
        }
        capture.onReadinessChanged = { [weak self] ready in
            guard let self, self.session.phase == .recording || self.session.phase == .listening else { return }
            if ready { self.updateRecordingOverlay() } else { self.overlay?.preparing(self.session.phase) }
        }
        hotkey = Hotkey()
        hotkey?.onAction = { [weak self] in self?.handleShortcut($0) }
        speech.canOffer = { [weak self] in
            guard let self else { return false }
            return self.conversation == nil && self.waitingPaste == nil && self.session.phase != .processing && (self.session.phase != .recording || self.autoTarget != nil)
        }
        speech.willPlay = { [weak self] in
            guard let self else { return false }
            guard self.conversation == nil, self.waitingPaste == nil && self.session.phase != .processing && (self.session.phase != .recording || self.autoTarget != nil) else {
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
        speech.onActivityChanged = { [weak self] active in
            guard let self else { return }
            self.hotkey?.captureEscape(self.openingConversation || active || self.waitingPaste != nil || self.session.phase == .listening || self.session.phase == .recording || self.session.phase == .processing)
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
        let menu = NSMenu()
        for (title, action) in [("Dictation History…", #selector(showHistory)), ("Vocabulary…", #selector(openVocabulary)), ("Start Conversation · Hyper+C", #selector(startConversation)), ("Catch Up · Hyper+M", #selector(catchUp)), ("Paste Permissions…", #selector(showAccessibilitySetup)), ("Start / Stop Dictation", #selector(toggle)), ("Cancel Dictation", #selector(cancelDictation)), ("Hide Pill", #selector(toggleOverlay)), ("Quit Hey Talk", #selector(quit))] {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: ""); item.target = self; menu.addItem(item)
            if action == #selector(toggleOverlay) { overlayMenuItem = item }
        }
        menu.insertItem(selectionCapture.menuItem(), at: 4)
        menuItem?.menu = menu
        let autoItem = NSMenuItem(title: "Auto Mode · Hyper+V", action: #selector(toggleAutoMode), keyEquivalent: "")
        autoItem.target = self; menu.insertItem(autoItem, at: 2); autoMenuItem = autoItem
        let correctionItem = NSMenuItem(title: "AI Correction", action: #selector(toggleCorrection), keyEquivalent: "")
        correctionItem.target = self; menu.insertItem(correctionItem, at: 3); correctionMenuItem = correctionItem
        let returnItem = NSMenuItem(title: "Return to Field Before Pasting", action: #selector(toggleReturnToField), keyEquivalent: "")
        returnItem.target = self; returnItem.state = returnToField ? .on : .off
        menu.insertItem(returnItem, at: 4); returnMenuItem = returnItem
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
                return self.automatic?.followsFocus == true ? input != nil : input == target
            }
            if self.session.phase == .listening || (self.session.phase == .idle && !self.autoSubmitKey.passNextEnter) { return false }
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
        let vocabularyTimer = DispatchSource.makeTimerSource(queue: .main)
        vocabularyTimer.schedule(deadline: .now(), repeating: .seconds(15), leeway: .seconds(3))
        vocabularyTimer.setEventHandler { [weak self] in self?.refreshVocabulary() }
        vocabularyTimer.resume(); self.vocabularyTimer = vocabularyTimer
        showOverlay(overlayEnabled)
        refreshConversation()
        applyAutoMode()
        if CommandLine.arguments.contains("--show-history") { showHistory() }
        for number in [SIGTERM, SIGINT] {
            signal(number, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: number, queue: .main)
            source.setEventHandler { NSApp.terminate(nil) }; source.resume(); signals.append(source)
        }
        NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main) { [weak self] _ in self?.cancel(keepMicrophoneWarm: false) }
    }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard pausedConversation != nil || switchingToDictation else { return .terminateNow }
        quitAfterHandoff = true
        if pausedConversation != nil { cancel(keepMicrophoneWarm: false) }
        return .terminateLater
    }
    func applicationWillTerminate(_ notification: Notification) { selectionCapture.stop(); speech.shutdown(); vocabularyTimer?.cancel(); focusMonitor.stop(); autoSubmitKey.stop(); capture.cancel(); inference.cancel(); correction.cancel(); history.finishPendingWrites() }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        showOverlay(true); showHistory(); return true
    }
    @objc private func showAccessibilitySetup() { accessibilitySetup.show() }
    @objc private func openVocabulary() { editVocabulary(nil) }
    private func editVocabulary(_ term: String?) {
        vocabularyWindow.show(term: term) { [weak self] args, reply in self?.refreshVocabulary(args, reply: reply) }
    }
    @objc private func showHistory() {
        history.refresh()
        historyWindow.show(keepingFocus: session.phase == .recording)
    }
    @objc private func startConversation() { openConversation() }
    @objc private func catchUp() {
        Task { @MainActor in
            do { try await startCatchUp() }
            catch { speech.onNotice?(error.localizedDescription) }
        }
    }
    @MainActor private func startCatchUp() async throws {
        try await speech.catchUp { [weak self] recap in self?.openConversation(recap: recap) }
    }
    private func openConversation(recap: String? = nil) {
        guard !openingConversation, !switchingToDictation else { return }
        guard session.phase != .processing, waitingPaste == nil,
              session.phase != .recording || autoTarget != nil else {
            showOverlay(true); overlay?.error("Finish dictating before opening chat."); return
        }
        if autoTarget != nil || autoRestart != nil { cancel() }
        if speech.active { speech.stop() }
        conversationFocus = ConversationFocus()
        openingConversation = true
        showOverlay(true)
        applyConversation(["active": true, "phase": "starting", "message": "Opening chat…", "action": "Cancel opening chat"])
        let revision = conversationRevision
        Task { @MainActor in
            do {
                guard openingConversation, conversationRevision == revision else { return }
                var request: [String: Any] = ["command": recap == nil ? "conversation_start" : "conversation_context", "sync": false]
                if let recap { request["title"] = "Catch Up"; request["description"] = recap }
                // Queue Stop behind Start if Escape arrives while the socket request is in flight.
                let reply: [String: Any]? = try await withCheckedThrowingContinuation { continuation in
                    contextQueue.async { [request] in continuation.resume(with: Result { try self.chatRequest(request) }) }
                }
                guard openingConversation, conversationRevision == revision else { return }
                guard reply?["status"] as? String == "ok" else { throw TalkError("Update Hey Boss on macOS 26 to use voice chat.") }
                openingConversation = false; refreshConversation()
            } catch {
                guard openingConversation, conversationRevision == revision else { return }
                openingConversation = false; applyConversation(nil)
                showOverlay(true); overlay?.error("Couldn’t open chat: " + error.localizedDescription)
            }
        }
    }
    static func requestDesktop(_ request: [String: Any]) throws -> [String: Any]? {
        let job = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/LaunchAgents/local.hey-boss.plist")
        guard let sockets = NSDictionary(contentsOf: job)?["Sockets"] as? [String: [String: Any]],
              let path = sockets["Listener"]?["SockPathName"] as? String else {
            throw TalkError("Install Hey Boss on this Mac to open chat.")
        }
        let fd = try ControlSocket.connect(path: path)
        defer { close(fd) }
        try Wire.write(Wire.json(request), to: fd, timeout: 5)
        shutdown(fd, SHUT_WR)
        let reply = try JSONSerialization.jsonObject(with: Wire.response(from: fd)) as? [String: Any]
        return reply
    }
    @objc private func conversationChanged(_ notification: Notification) {
        if let level = notification.userInfo?["level"] as? Double { conversationLevel = level; return }
        guard !openingConversation, !switchingToDictation else { return }
        guard let state = notification.userInfo as? [String: Any] else { return }
        applyConversation(state)
    }
    @objc private func conversationAppChanged(_ notification: Notification) {
        guard let app = notification.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication,
              app.bundleIdentifier == "local.hey-boss.desktop" else { return }
        refreshConversation()
    }
    private func refreshConversation() {
        guard !openingConversation, !switchingToDictation else { return }
        guard !NSRunningApplication.runningApplications(withBundleIdentifier: "local.hey-boss.desktop").isEmpty else { applyConversation(nil); return }
        let revision = conversationRevision
        contextQueue.async {
            guard let state = (try? self.chatRequest(["command": "conversation_status", "sync": false]))?["conversation"] as? [String: Any] else { return }
            DispatchQueue.main.async {
                guard self.conversationRevision == revision, !self.openingConversation, !self.switchingToDictation else { return }
                self.applyConversation(state)
            }
        }
    }
    func applyConversation(_ state: [String: Any]?) {
        conversationRevision += 1
        selectionCapture.conversation = state?["selection_chat"] as? String
        let next = state?["active"] as? Bool == true ? state : nil
        if next != nil, waitingPaste != nil || session.phase == .processing || (session.phase == .recording && autoTarget == nil) {
            if next?["microphone"] as? Bool == true && resumedDuringProcessing != session.generation {
                contextQueue.async { _ = try? self.chatRequest(["command": "conversation_control", "title": "dictation_busy", "sync": false]) }
            }
            return
        }
        if next != nil, conversation == nil {
            if autoTarget != nil || autoRestart != nil { cancel() }
            if speech.active { speech.stop() }
        }
        if next?["phase"] as? String == "recording", conversation?["phase"] as? String != "recording",
           let app = FocusedInput.keyboardApp, app.bundleIdentifier != "local.hey-boss.desktop" {
            conversationFocus = ConversationFocus(app: app)
        }
        let previous = conversation != nil
        conversation = next; conversationLevel = 0
        if next != nil || previous {
            overlay?.showConversation(next)
            if next == nil { overlay?.update(session.phase) }
        }
        if next == nil, previous {
            conversationFocus = nil
            if let detail = state?["detail"] as? String, state?["phase"] as? String == "idle",
               !["Ready", "Voice off", "Microphone off"].contains(detail) { overlay?.error(detail) }
            focusTrigger.reset()
            DispatchQueue.main.async { [weak self] in
                guard let self, self.conversation == nil else { return }
                self.focusedInputChanged(FocusedInput.current(bundleIDs: self.autoSettings.bundleIDs))
            }
        }
        hotkey?.captureEscape(openingConversation || speech.active || waitingPaste != nil || session.phase == .listening || session.phase == .recording || session.phase == .processing)
    }
    private func switchToDictation() {
        guard !openingConversation, !switchingToDictation else { return }
        guard pausedConversation == nil else { return }
        switchingToDictation = true
        contextQueue.async {
            let result = try? self.chatRequest(["command": "conversation_pause", "sync": false])
            DispatchQueue.main.async {
                self.switchingToDictation = false
                guard result?["status"] as? String == "ok", let pause = result?["pause_id"] as? String else {
                    if self.quitAfterHandoff { NSApp.reply(toApplicationShouldTerminate: true); return }
                    self.refreshConversation()
                    self.overlay?.error("Couldn’t switch to dictation."); return
                }
                self.pausedConversation = pause
                if self.quitAfterHandoff { self.returnMicrophoneToConversation(); return }
                self.applyConversation(nil)
                self.start(showOverlay: self.overlayEnabled, paste: true) { result in
                    if result["ok"] as? Bool != true { self.returnMicrophoneToConversation() }
                }
            }
        }
    }
    private func returnMicrophoneToConversation() {
        guard let pause = pausedConversation else { return }
        pausedConversation = nil
        switchingToDictation = true
        resumedDuringProcessing = session.generation
        capture.releaseMicrophone {
            self.contextQueue.async {
                let result = try? self.chatRequest(["command": "conversation_resume", "pause_id": pause, "sync": false])
                DispatchQueue.main.async {
                    self.switchingToDictation = false
                    if self.quitAfterHandoff { NSApp.reply(toApplicationShouldTerminate: true); return }
                    if result?["status"] as? String == "ok" { self.applyConversation(result?["conversation"] as? [String: Any]) }
                    else { self.refreshConversation(); self.overlay?.error("Couldn’t resume the paused conversation.") }
                }
            }
        }
    }
    private func controlConversation(_ action: String = "toggle") {
        guard !switchingToDictation else { return }
        if openingConversation {
            openingConversation = false; applyConversation(nil)
            contextQueue.async { _ = try? self.chatRequest(["command": "conversation_control", "title": "stop", "sync": false]) }
            return
        }
        if conversation?["phase"] as? String == "idle", action == "toggle" {
            var starting = conversation ?? [:]
            starting["phase"] = "starting"; starting["message"] = "Starting…"; starting["action"] = "Stop voice"
            applyConversation(starting)
        }
        let revision = conversationRevision
        contextQueue.async {
            let result = try? self.chatRequest(["command": "conversation_control", "title": action, "sync": false])
            DispatchQueue.main.async {
                guard self.conversationRevision == revision else { return }
                self.applyConversation(result?["conversation"] as? [String: Any])
            }
        }
    }
    func handleShortcut(_ action: HotkeyAction) {
        switch action {
        case .toggle(.submit): recordShortcut()
        case .toggle(.autoSubmit): toggleAutoSubmit()
        case .toggleAuto: toggleAutoMode()
        case .toggleSpeech: speech.toggle()
        case .toggleSpeechMode: speech.toggleEnabled()
        case .startConversation:
            if conversation == nil { startConversation() }
        case .catchUp: catchUp()
        case .cancel:
            if openingConversation { controlConversation("stop"); return }
            guard conversation == nil else { return }
            if speech.active { speech.stop() }
            else if session.phase == .recording || session.phase == .processing || autoTarget != nil || autoRestart != nil || waitingPaste != nil { cancelDictation() }
        case .none: break
        }
    }
    private func recordShortcut() {
        guard !openingConversation, !switchingToDictation else { return }
        if conversation != nil { switchToDictation(); return }
        guard let app = NSWorkspace.shared.frontmostApplication, app.bundleIdentifier == "local.hey-boss.desktop",
              session.phase == .idle || session.phase == .failed else { toggle(); return }
        contextQueue.async {
            // The transcript can gain focus before its status notification arrives.
            let state = (try? self.chatRequest(["command": "conversation_status", "sync": false]))?["conversation"] as? [String: Any]
            DispatchQueue.main.async {
                guard NSWorkspace.shared.frontmostApplication == app else { return }
                if state?["active"] as? Bool == true || self.conversation != nil { self.switchToDictation() }
                else { self.toggle() }
            }
        }
    }
    private func showOverlay(_ show: Bool) {
        if overlay == nil {
            let view = Overlay(selectionCapture: selectionCapture)
            view.onCancelPaste = { [weak self] in self?.cancelDictation() }
            view.onToggle = { [weak self] in self?.toggle() }
            view.onToggleSpeech = { [weak self] in self?.speech.toggleEnabled() }
            view.onToggleAuto = { [weak self] in self?.toggleAutoMode() }
            view.onToggleAutoSubmit = { [weak self] in self?.toggleAutoSubmit() }
            view.onToggleReturnToField = { [weak self] in self?.toggleReturnToField() }
            view.returnToFieldEnabled = { [weak self] in self?.returnToField ?? false }
            view.onToggleCorrection = { [weak self] in self?.toggleCorrection() }
            view.onEditVocabulary = { [weak self] term in self?.editVocabulary(term) }
            view.onShowHistory = { [weak self] in self?.showHistory() }
            view.onStartConversation = { [weak self] in self?.startConversation() }
            view.onPauseListening = { [weak self] paused in self?.controlConversation(paused ? "pause_listening" : "resume_listening") }
            view.onOpenTranscript = { [weak self] in self?.controlConversation("show") }
            view.onCatchUp = { [weak self] in self?.catchUp() }
            view.onInterruptConversation = { [weak self] in self?.controlConversation("interrupt") }
            view.onStopConversation = { [weak self] in self?.controlConversation("stop") }
            view.speechEnabled = { [weak self] in self?.speech.settings.enabled ?? false }
            view.onOpenAccessibility = { [weak self] in
                self?.accessibilitySetup.show()
                self?.accessibilitySetup.openSettings()
            }
            view.level = { [weak self] in
                guard let self else { return 0 }
                return self.conversation == nil ? self.capture.level : self.conversationLevel
            }
            view.setAutoMode(autoSettings.enabled)
            view.setCorrection(correctionSettings.enabled)
            overlay = view
        }
        overlayEnabled = show; overlay?.show(show)
        overlayMenuItem?.title = show ? "Hide Pill" : "Show Pill"
        updateStatusTooltip()
        DispatchQueue.main.async { [weak self] in self?.updateSubmitIndicator() }
    }
    @objc private func toggleOverlay() { showOverlay(!overlayEnabled) }
    @objc private func updateSubmitIndicator() {
        let active = session.phase == .recording || session.phase == .processing
        let app = active ? target : FocusedInput.keyboardApp
        let requested = !active || (paste && (pressEnter || (automatic != nil && session.phase == .recording)))
        let followsFocus = session.phase == .recording && automatic?.followsFocus == true
        let host = active && !followsFocus ? recordingHost : BrowserPage.host(in: app)
        autoSubmitEnabled = (!BrowserPage.isBrowser(app) || host != nil) &&
            enterExceptions.shouldPressEnter(requested: requested, bundleID: app?.bundleIdentifier, host: host)
        overlay?.setAutoSubmit(autoSubmitEnabled)
    }
    private func updateStatusTooltip() {
        let text = "Hey Talk is running · Pill \(overlayEnabled ? "visible" : "hidden")"
        menuItem?.button?.toolTip = text
        menuItem?.button?.setAccessibilityLabel(text)
    }
    @objc private func toggleReturnToField() {
        UserDefaults.standard.set(!returnToField, forKey: "returnToField")
        returnMenuItem?.state = returnToField ? .on : .off
    }
    @objc private func toggleCorrection() {
        try? correctionSettings.command(["toggle"])
        applyCorrection()
        overlay?.showStatus(correctionSettings.enabled ? "AI correction on" : "AI correction off")
    }
    private func applyCorrection() {
        correctionMenuItem?.state = correctionSettings.enabled ? .on : .off
        overlay?.setCorrection(correctionSettings.enabled)
    }
    @objc private func toggleAutoMode() {
        autoSettings.enabled.toggle()
        showOverlay(true)
        applyAutoMode()
        if !autoSettings.enabled || autoSubmitKey.ready { overlay?.showStatus(autoSettings.enabled ? "Auto mode on" : "Auto mode off") }
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
        guard conversation == nil, !openingConversation, !switchingToDictation else { return }
        if input != autoTarget { autoSubmitKey.passNextEnter = false }
        if session.phase == .listening, input != autoTarget {
            cancel()
            focusTrigger.reset()
        }
        if automatic != nil {
            automatic?.focusChanged(input, at: ProcessInfo.processInfo.systemUptime)
            if session.phase == .recording {
                if input != nil && (automatic?.followsFocus == true || input == autoTarget) {
                    focusGrace?.cancel(); focusGrace = nil
                    target = autoTarget?.app; targetName = target?.localizedName
                    if capture.speechComplete { stop { _ in } }
                } else if focusGrace == nil {
                    let id = session.generation
                    let timeout = DispatchWorkItem { [weak self] in
                        guard let self, self.session.generation == id, self.session.phase == .recording else { return }
                        self.focusMonitor.refresh()
                        if self.automatic?.shouldCancel(at: ProcessInfo.processInfo.systemUptime) == true {
                            if self.automatic?.followsFocus == false { self.stop { _ in } } else { self.cancel() }
                        }
                    }
                    focusGrace = timeout
                    DispatchQueue.main.asyncAfter(deadline: .now() + AutoRecording<FocusedInput>.focusGrace, execute: timeout)
                }
            }
        }
        let available = waitingPaste == nil && !speech.active && autoSettings.enabled && autoTarget == nil && autoRestart == nil && (session.phase == .idle || session.phase == .failed)
        guard focusTrigger.update(input, available: available), let input, autoSubmitKey.start() else { return }
        start(showOverlay: true, paste: true, pressEnter: true, focusedInput: input) { [weak self] result in
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
        if let conversation {
            let action = conversation["action"] as? String
            controlConversation(action == "Stop voice" || action == "Stop reply" ? "stop" : "toggle")
            return
        }
        switch DictationToggle.resolve(phase: session.phase, currentSubmit: pressEnter, requestedSubmit: true) {
        case .stop(let submit): pressEnter = submit; stop { _ in }
        case .start(let submit): start(showOverlay: overlayEnabled, paste: true, pressEnter: submit) { _ in }
        case .busy: break
        }
    }
    private func toggleAutoSubmit() {
        guard let app = FocusedInput.keyboardApp, let bundleID = app.bundleIdentifier else { return }
        do {
            if BrowserPage.isBrowser(app) {
                guard !enterExceptions.bundleIDs.contains(bundleID) else {
                    overlay?.error("Auto Submit is off for the whole browser."); return
                }
                guard let host = BrowserPage.host(in: app) else { overlay?.error("No website identified."); return }
                if let rule = enterExceptions.excludedSite(host), rule != host {
                    overlay?.error("Excluded by " + rule + ". Remove it in enter-exceptions sites."); return
                }
                _ = try enterExceptions.command(["sites", enterExceptions.sites.contains(host) ? "remove" : "add", host])
            } else {
                _ = try enterExceptions.command([enterExceptions.bundleIDs.contains(bundleID) ? "remove" : "add", bundleID])
            }
            updateSubmitIndicator()
        } catch { overlay?.error(error.localizedDescription) }
    }
    @objc private func cancelDictation() {
        guard conversation == nil else { return }
        if let wait = waitingPaste, wait.timer.isValid,
           !Paste.copyAndStopWaiting(wait.text, retry: wait.timer, to: .general) {
            overlay?.waitToPaste("Clipboard unavailable. Try copying again.", while: wait.timer)
            return
        }
        cancel()
    }
    @objc private func quit() { NSApp.terminate(nil) }

    private func status() -> [String: Any] {
        var value: [String: Any] = [
            "ok": true, "state": session.phase.rawValue, "pid": getpid(),
            "model": "orukeet-v0.1.0", "model_ready": ModelPaths.available,
            "hotkey_mode": "toggle", "hotkey": DictationShortcut.submit.name, "hotkey_registered": hotkey?.registered ?? false,
            "hotkeys": ["auto_submit": DictationShortcut.autoSubmit.name, "submit": DictationShortcut.submit.name, "auto": "Control+Command+Option+Shift+V", "conversation": "Control+Command+Option+Shift+C", "context": "Control+Command+Option+Shift+M"],
            "enter_exceptions": enterExceptions.bundleIDs, "enter_exception_sites": enterExceptions.sites,
            "auto": autoSettings.json, "auto_ready": autoSubmitKey.ready,
            "correction": correctionSettings.json,
            "waiting_to_paste": waitingPaste != nil, "auto_recording": autoTarget != nil && session.phase == .recording,
            "auto_listening": session.phase == .listening, "auto_focused": focusMonitor.current != nil,
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
        value["conversation"] = conversation
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
    func command(_ request: [String: Any], reply: @escaping ([String: Any]) -> Void) {
        let command = request["command"] as? String ?? ""
        switch command {
        case "deck": deck(request["action"] as? String, reply: reply)
        case "remember_focus":
            if conversation == nil || conversationFocus == nil { conversationFocus = ConversationFocus() }
            reply(["ok": true])
        case "spoken_context":
            do {
                var result: [String: Any] = ["ok": true, "paste_tool": try Correction.pasteTool]
                if let context = conversationFocus { result["id"] = context.id; result["observed"] = context.evidence }
                reply(result)
            } catch { reply(failure(error.localizedDescription)) }
        case "paste_text", "spoken_window":
            if command == "paste_text", !AXIsProcessTrusted() { reply(failure(ConversationFocus.permission)); return }
            guard let context = conversationFocus, request["focus"] as? String == context.id else {
                reply(failure("The original focus is unavailable or has changed.")); return
            }
            Task { @MainActor in
                do {
                    if command == "paste_text" {
                        guard let text = request["text"] as? String, !text.isEmpty else { throw TalkError("Missing text.") }
                        try await context.paste(text) { self.conversationFocus?.id == context.id }
                        reply(["ok": true])
                    } else {
                        guard context.canInspectWindow, let window = context.window else { throw TalkError("Original window unavailable or protected.") }
                        let jpeg = try await WindowInspection.screenshot(context.app, window: window, visible: [])
                        guard self.conversationFocus?.id == context.id, context.canInspectWindow else { throw TalkError("Original window changed or is protected.") }
                        reply(["ok": true, "jpeg": jpeg, "source": "Current view of the previously observed window"])
                    }
                } catch { reply(failure(error.localizedDescription)) }
            }
        case "inspect_window":
            guard let target = request["target"] as? String, let mode = request["mode"] as? String else { reply(failure("Missing target or format.")); return }
            Task {
                do { reply(try await WindowInspection.capture(target: target, mode: mode)) }
                catch { reply(failure(error.localizedDescription)) }
            }
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
                case "catch-up":
                    Task { @MainActor in
                        do { try await startCatchUp(); reply(["ok": true, "speech": speech.status]) }
                        catch { speech.onNotice?(error.localizedDescription); reply(failure(error.localizedDescription)) }
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
                    if !autoSettings.enabled || autoSubmitKey.ready { overlay?.showStatus(autoSettings.enabled ? "Auto mode on" : "Auto mode off") }
                }
                reply(["ok": true, "auto": autoSettings.json, "auto_ready": autoSubmitKey.ready])
            } catch { reply(failure(error.localizedDescription)) }
        case "vocabulary":
            guard let arguments = request["arguments"] as? [String] else { reply(failure("Missing vocabulary arguments")); return }
            refreshVocabulary(arguments, reply: reply)
        case "enter-exceptions":
            do {
                guard let arguments = request["arguments"] as? [String] else { throw TalkError("Missing exception arguments") }
                let exceptions = try enterExceptions.command(arguments)
                updateSubmitIndicator()
                reply(["ok": true, arguments.first == "sites" ? "enter_exception_sites" : "enter_exceptions": exceptions])
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
        case "cancel": cancelDictation(); reply(status())
        case "last":
            if let entry = history.entries.first, entry.createdAt >= HistoryStore.cutoff || history.unsaved.contains(entry.id) { reply(entry.json.merging(["ok": true]) { _, new in new }) }
            else {
                history.read { result in
                    switch result {
                    case .success(let entries): reply((entries.first?.json ?? ["text": "", "original_text": NSNull(), "app_context": NSNull()]).merging(["ok": true]) { _, new in new })
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
                paste = false; pressEnter = false; transcriptionSource = "file"; error = nil; pendingReply = reply
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
        guard conversation == nil, !switchingToDictation else { reply(failure("Stop voice chat before starting dictation.")); return }
        if session.phase == .listening { cancel() }
        if session.phase == .idle || session.phase == .failed {
            self.showOverlay(showOverlay)
            overlay?.preparing(focusedInput == nil ? .recording : .listening)
        }
        let id: UInt64
        do { id = try session.begin(listening: focusedInput != nil) } catch { reply(failure(error.localizedDescription)); return }
        speech.stop()
        pendingStartReply = reply
        error = nil; captureReady = false; captureMilliseconds = 0
        self.paste = paste; self.pressEnter = pressEnter; transcriptionSource = "microphone"
        clearAutomatic()
        automatic = focusedInput.map { AutoRecording(target: $0, followsFocus: $0.app.bundleIdentifier != "com.t3tools.t3code") }
        target = focusedInput?.app ?? FocusedInput.keyboardApp
        slackTeam = paste && correctionSettings.enabled ? SlackTools.route(target)?.pathComponents[2] : nil
        recordingHost = BrowserPage.host(in: target)
        targetName = target?.localizedName
        if let target, (target.bundleIdentifier == "com.t3tools.t3code" && (paste || correctionSettings.enabled)) || (paste && (returnToField || correctionSettings.enabled)) {
            startingInput = focusedInput ?? FocusedInput.read(app: target, forAuto: false)
            if paste && returnToField { startingInput?.rememberCmuxTab() }
        } else { startingInput = nil }
        if paste {
            Paste.log.notice("Capture: target=\(self.target?.bundleIdentifier ?? "none", privacy: .public), field=\(self.startingInput != nil), return=\(self.returnToField)")
        }
        idleRelease?.cancel(); hotkey?.captureEscape(true)
        let begin: () -> Void = {
            guard self.session.generation == id, self.session.phase == .recording || self.session.phase == .listening else { return }
            let start = ProcessInfo.processInfo.systemUptime
            self.inference.warm()
            self.capture.start(uid: self.inputUID, detectSpeech: focusedInput != nil) { result in
                guard self.session.generation == id, self.session.phase == .recording || self.session.phase == .listening else { return }
                switch result {
                case .failure(let error): self.fail(error.localizedDescription)
                case .success:
                self.captureMilliseconds = (ProcessInfo.processInfo.systemUptime - start) * 1000
                self.captureReady = true
                self.updateRecordingOverlay(); self.hotkey?.captureEscape(true)
                if focusedInput == nil { self.recordingBegan() }
                self.pendingStartReply?(self.status()); self.pendingStartReply = nil
                }
            }
        }
        if AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
            AVCaptureDevice.requestAccess(for: .audio) { _ in DispatchQueue.main.async(execute: begin) }
        } else { begin() }
    }
    private func recordingBegan() {
        previousCommands = commands.beginRecording()
        let id = session.generation
        updateRecordingOverlay()
        if self.correctionSettings.enabled, let target = self.target {
            let pid = target.processIdentifier
            let isT3 = target.bundleIdentifier == "com.t3tools.t3code"
            self.recordingContext = Task.detached(priority: .userInitiated) { [startingInput = self.startingInput] in
                guard !Task.isCancelled, FocusedInput.keyboardApp?.processIdentifier == pid,
                      !isT3 || startingInput?.isFocused() == true else { return (nil, 0) }
                let started = ProcessInfo.processInfo.systemUptime
                let text = AppContext.capture(pid: pid)
                let elapsed = (ProcessInfo.processInfo.systemUptime - started) * 1000
                guard !isT3 || startingInput?.isFocused() == true else { return (nil, elapsed) }
                return (text, elapsed)
            }
        }
        let limit = DispatchWorkItem { [weak self] in
            guard let self, self.session.generation == id, self.session.phase == .recording else { return }
            if self.autoTarget != nil {
                self.cancel(); self.overlay?.error("Auto timed out. Focus the message field again.")
            } else { self.stop { _ in } }
        }
        self.maximumDuration = limit
        DispatchQueue.main.asyncAfter(deadline: .now() + 299, execute: limit)
    }
    private func updateRecordingOverlay() {
        overlay?.update(session.phase)
        if capture.fallbackSource != nil, let input = capture.activeInput {
            overlay?.error("Using \(input.name). Bluetooth microphone unavailable.")
        }
    }
    private func stop(_ reply: @escaping ([String: Any]) -> Void) {
        if session.phase == .listening { cancel(); reply(status()); return }
        guard session.phase == .recording else { reply(failure("No recording is active")); return }
        guard captureReady else {
            // An early Enter must not cancel Auto while the microphone opens.
            if autoTarget == nil { cancel() }
            reply(failure("Stopped before the microphone was ready")); return
        }
        if automatic != nil {
            var input = FocusedInput.current(bundleIDs: autoSettings.bundleIDs)
            if returnToField { input?.rememberCmuxTab() }
            guard automatic?.followsFocus == false || input != nil else {
                reply(failure("Focus an allowed input before finishing Auto dictation")); return
            }
            if automatic?.followsFocus == true {
                recordingHost = BrowserPage.host(in: input?.app)
                slackTeam = SlackTools.route(input?.app)?.pathComponents[2]
            }
            automatic?.finish(in: input)
            target = autoTarget?.app; targetName = target?.localizedName
        }
        autoSubmitKey.passNextEnter = false
        focusGrace?.cancel(); focusGrace = nil
        maximumDuration?.cancel(); maximumDuration = nil
        let id = session.generation
        captureReady = false
        do { try session.processing(id) } catch { reply(failure(error.localizedDescription)); return }
        overlay?.update(.processing); pendingReply = reply
        let snapshot = recordingContext
        recordingContext = nil
        capture.stop { result in
            guard self.session.generation == id else { return }
            self.returnMicrophoneToConversation()
            switch result {
            case .failure(let error): self.fail(error.localizedDescription)
            case .success(let samples):
                self.history.saveRecording(samples)
                guard samples.count >= 2400 else {
                    if self.autoTarget != nil { self.noSpeechDetected(id: id) } else { self.cancel() }
                    return
                }
                self.decode(samples, id: id, snapshot: snapshot)
            }
        }
    }
    private func decode(_ samples: [Float], id: UInt64, snapshot: ContextSnapshot? = nil) {
        let audioSeconds = Double(samples.count) / 16_000
        let useCorrection = correctionSettings.enabled
        overlay?.processing(audioSeconds: audioSeconds, correction: useCorrection)
        hotkey?.captureEscape(true)
        inference.transcribe(samples) { result in Task { @MainActor in
            let context = useCorrection ? await snapshot?.value : nil
            guard self.session.generation == id else { return }
            switch result {
            case .failure(let error): self.fail(error.localizedDescription)
            case .success(let (text, elapsed)):
                guard !text.isEmpty else { self.noSpeechDetected(id: id); return }
                self.deliverTranscript(text, context: context, correctionEnabled: useCorrection, id: id,
                                       audioSeconds: audioSeconds, elapsed: elapsed)
            }
        } }
    }

    private func deliverTranscript(_ original: String, context: (text: String?, milliseconds: Double)?, correctionEnabled: Bool,
                                   id generation: UInt64, audioSeconds: Double, elapsed: Double) {
        var id = generation
        var started = ProcessInfo.processInfo.systemUptime
        let target = self.target, host = recordingHost, automatic = autoTarget
        var field = automatic ?? startingInput
        let needsField = target?.bundleIdentifier == "com.t3tools.t3code" || returnToField || automatic != nil
        let current = { self.session.generation == id && self.session.phase == .processing }
        func allowsSubmit() -> Bool {
            enterExceptions.shouldPressEnter(requested: pressEnter, bundleID: target?.bundleIdentifier, host: host) &&
                (!BrowserPage.isBrowser(target) || (host != nil && BrowserPage.host(in: target) == host))
        }

        var response: [String: Any] = [:]
        var copiedOriginal = false
        var info: [String: Any] = ["status": correctionEnabled ? "applied" : "off",
                                   "context_ms": context?.milliseconds ?? 0, "context_characters": context?.text?.count ?? 0]
        func report(_ delivery: String, _ reason: PasteBlockReason? = nil) {
            if self.paste {
                Paste.log.notice("Delivery: \(delivery, privacy: .public), reason=\(reason?.rawValue ?? "none", privacy: .public), correction=\(info["status"] as? String ?? "off", privacy: .public)")
            }
            response["delivery"] = delivery; response["paste_block"] = reason?.rawValue
            if correctionEnabled { info["elapsed_ms"] = (ProcessInfo.processInfo.systemUptime - started) * 1000 }
            response["correction"] = info
            let reply = pendingReply; pendingReply = nil; reply?(response)
        }
        func finish(_ delivery: String, _ reason: PasteBlockReason? = nil, action: CorrectionAction? = nil, retry: (() -> Void)? = nil) {
            guard current(), session.finish(id, text: response["text"] as? String ?? original) else { return }
            report(delivery, reason)
            waitingPaste = nil; self.automatic = nil; hotkey?.reset()
            overlay?.finishProcessing(); scheduleRelease()
            lastPasteBlock = reason
            if response["history_saved"] as? Bool == false { overlay?.error(copiedOriginal ? "Raw transcript copied. History couldn’t be saved." : "Not saved. Copy from History before quitting.", retry: retry) }
            else if reason == .slackUnavailable { overlay?.showActivity(info["error"] as? String ?? "Slack unavailable", failed: true) }
            else if let action, reason == .t3Unavailable || reason == .newThreadUnavailable {
                overlay?.confirmAction(action, failed: true)
            }
            else if let reason { overlay?.error(reason.message(targetName: targetName, pasted: delivery == "pasted"), accessibilityAction: reason == .accessibility, retry: retry) }
            else if let error = info["error"] as? String { overlay?.error(copiedOriginal ? "Raw transcript copied. " + error : error, retry: retry) }
            if automatic != nil, reason == nil, info["error"] == nil, ["submitted", "switched", "pinned", "unpinned", "settled", "navigated", "vocabulary_added"].contains(delivery) {
                restartAutomatic(id: id)
            }
        }
        // Keep each tool's text recoverable before touching the original field.
        func save(_ action: CorrectionAction, then: @escaping () -> Void) {
            let entry = Dictation(text: action.text, originalText: original, audioSeconds: audioSeconds,
                                  source: transcriptionSource, appContext: context?.text)
            history.save(entry) { saved in
                guard current() else { return }
                response = ["ok": true, "text": action.text, "original_text": original, "audio_seconds": audioSeconds,
                            "app_context": context?.text as Any? ?? NSNull(), "inference_ms": elapsed * 1000,
                            "real_time_factor": elapsed / audioSeconds, "capture_start_ms": self.captureMilliseconds,
                            "history_id": entry.id.uuidString, "history_saved": saved]
                then()
            }
        }

        var lastDelivery: String?
        var listedThreads: [T3ThreadRow]?
        func perform(_ action: CorrectionAction, next: @escaping (String) -> Void) {
            guard current() else { return }
            if action == .listThreads {
                guard let rows = field?.threadRows() else {
                    save(.paste(original)) { finish("history", .t3Unavailable, action: action) }; return
                }
                listedThreads = rows
                let context: [String: Any] = ["current_thread": field?.composerLabel?.replacingOccurrences(of: "Message ", with: "", options: .anchored) ?? "",
                    "threads": rows.enumerated().map { index, row in
                        ["position": index + 1, "title": row.title, "description": row.label, "current": row.current] as [String: Any]
                    }]
                guard let data = try? JSONSerialization.data(withJSONObject: context, options: [.sortedKeys]) else { return }
                commands.executed(action)
                next(String(decoding: data, as: UTF8.self)); return
            }
            let originalField = field
            save(action) {
                guard self.paste else { finish("stdout"); return }
                if case .addVocabularyTerm(let term) = action {
                    self.refreshVocabulary(["add", term]) { result in
                        guard current() else { return }
                        guard result["ok"] as? Bool == true else {
                            let message = result["error"] as? String ?? "Couldn’t save vocabulary."
                            info["error"] = message; info["status"] = "failed"
                            response["ok"] = false; response["error"] = message
                            finish("history"); return
                        }
                        self.commands.executed(action)
                        self.overlay?.confirmAction(action)
                        lastDelivery = "vocabulary_added"
                        next("vocabulary_added")
                    }
                    return
                }
                if case .slack(let command) = action {
                    Task { @MainActor in
                        do {
                            guard let target, let team = self.slackTeam else { throw TalkError("Focus Slack") }
                            try await SlackTools.navigate(command, app: target, team: team, isCurrent: current)
                            guard current() else { return }
                            self.commands.executed(action)
                            self.overlay?.confirmAction(action)
                            lastDelivery = "navigated"
                            next("navigated")
                        } catch {
                            guard current() else { return }
                            info["error"] = (error as? TalkError)?.localizedDescription ?? "Slack unavailable"
                            info["status"] = "failed"
                            finish("history", .slackUnavailable, action: action)
                        }
                    }
                    return
                }
                Paste.deliver(action, target: target, originalField: originalField,
                              shouldPressEnter: allowsSubmit,
                              returnToField: self.returnToField ? originalField : nil, needsField: needsField,
                              onWait: lastDelivery == nil && (!needsField || originalField != nil) ? { timer in
                                  self.waitingPaste = (timer, action.text)
                                  self.hotkey?.captureEscape(true)
                                  self.overlay?.waitToPaste(originalField?.returnInstruction ?? "Return to \(self.targetName ?? "the original app")", while: timer)
                                  report("waiting")
                              } : nil, isCurrent: current, onNewField: { field = $0; listedThreads = nil }, listedThreads: listedThreads) { delivery, reason in
                    guard current() else { return }
                    if self.waitingPaste != nil {
                        self.waitingPaste = nil
                        self.overlay?.processing(audioSeconds: audioSeconds, correction: correctionEnabled)
                    }
                    guard correctionEnabled, reason == nil, delivery != "history" else { finish(delivery, reason, action: action); return }
                    self.commands.executed(action)
                    self.overlay?.confirmAction(action)
                    lastDelivery = delivery
                    next(delivery)
                }
            }
        }
        guard correctionEnabled else { perform(.paste(original)) { _ in }; return }
        correction.correct(original, vocabulary: (try? Vocabulary().words()) ?? [], appContext: context?.text,
                           app: {
                               guard self.transcriptionSource == "microphone", let target else { return nil }
                               var app = ["name": target.localizedName ?? "", "bundle_id": target.bundleIdentifier ?? ""]
                               if field?.canUseThreadActions == true {
                                   app["thread"] = field?.composerLabel?.replacingOccurrences(of: "Message ", with: "", options: .anchored)
                               }
                               return app
                           },
                           package: !paste || transcriptionSource != "microphone" ? nil : field?.canUseThreadActions == true ? .t3 : slackTeam != nil ? .slack : nil, canClear: paste && field != nil, canAddVocabularyTerm: paste,
                           recent: transcriptionSource == "microphone" ? previousCommands : [],
                           perform: paste ? perform : nil) { result in
            guard current() else { return }
            switch result {
            case .success(let action):
                if let delivery = lastDelivery { finish(delivery) }
                else { save(action) { finish("stdout") } }
            case .failure(let error):
                info["error"] = error.localizedDescription
                info["status"] = lastDelivery == nil ? "fallback" : "partial"
                func recover() {
                    let result = Paste.recoverTranscript(original, to: self.paste ? .general : nil)
                    copiedOriginal = result.delivery == "clipboard"
                    var retry: (() -> Void)?
                    if self.paste, let timeout = error as? CorrectionTimeout {
                        retry = { [finishedID = id] in
                            guard let next = self.session.retryProcessing(finishedID) else { return }
                            id = next; started = ProcessInfo.processInfo.systemUptime
                            copiedOriginal = false; info["status"] = "applied"; info["error"] = nil
                            self.idleRelease?.cancel(); self.hotkey?.captureEscape(true)
                            self.overlay?.processing(audioSeconds: audioSeconds, correction: true)
                            timeout.retry()
                        }
                    }
                    finish(lastDelivery ?? result.delivery, result.reason, retry: retry)
                }
                if lastDelivery != nil || response["history_saved"] as? Bool == true { recover() }
                else { save(.paste(original), then: recover) }
            }
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
        recordingContext?.cancel(); recordingContext = nil
        clearAutomatic()
        session.cancel(); maximumDuration?.cancel(); maximumDuration = nil
        capture.cancel(keepWarm: keepMicrophoneWarm); returnMicrophoneToConversation(); inference.cancel(); correction.cancel(); hotkey?.reset(); overlay?.update(.idle)
        pendingStartReply?(failure("Cancelled")); pendingStartReply = nil
        pendingReply?(failure("Cancelled")); pendingReply = nil
    }
    private func fail(_ message: String, keepMicrophoneWarm: Bool = false) {
        captureReady = false
        recordingContext?.cancel(); recordingContext = nil
        clearAutomatic()
        capture.cancel(keepWarm: keepMicrophoneWarm); returnMicrophoneToConversation(); session.fail(session.generation); hotkey?.reset()
        correction.cancel()
        maximumDuration?.cancel(); maximumDuration = nil
        error = message; overlay?.update(.failed); overlay?.error(OverlayNotice.concise(message))
        pendingStartReply?(failure(message)); pendingStartReply = nil
        pendingReply?(failure(message)); pendingReply = nil; scheduleRelease()
    }
    private func clearAutomatic() {
        if waitingPaste != nil { Paste.log.notice("Pending paste cancelled") }
        waitingPaste = nil
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

// The keys use the same actions and current state as the pill and keyboard.
// Polling must not enumerate audio devices, query Accessibility, or load a model.
private extension TalkService {
    func deck(_ action: String?, reply: @escaping ([String: Any]) -> Void) {
        if let action {
            switch action {
            case "dictation" where session.phase != .processing && waitingPaste == nil: recordShortcut()
            case "conversation" where conversation != nil: toggle()
            case "conversation", "new-chat":
                guard !openingConversation, !switchingToDictation, waitingPaste == nil,
                      session.phase != .processing, session.phase != .recording || autoTarget != nil
                else { reply(failure("Finish dictating first")); return }
                startConversation()
            case "mute" where conversation?["listeningPaused"] is Bool:
                controlConversation(conversation?["listeningPaused"] as? Bool == true ? "resume_listening" : "pause_listening")
            case "interrupt" where conversation?["interrupt"] as? Bool == true: controlConversation("interrupt")
            case "transcript" where conversation != nil: controlConversation("show")
            case "stop" where conversation != nil: controlConversation("stop")
            case "stop" where speech.active: speech.stop()
            case "stop" where session.phase != .idle || waitingPaste != nil: cancelDictation()
            case "send" where autoTarget != nil && session.phase == .recording && autoSubmitKey.shouldCapture():
                autoSubmitKey.onSubmit?()
            case "auto": toggleAutoMode()
            case "submit": toggleAutoSubmit()
            case "correction": toggleCorrection()
            case "return": toggleReturnToField()
            case "pill": toggleOverlay()
            case "speech": speech.toggleEnabled()
            case "play":
                speech.toggle { result in
                    switch result {
                    case .success: reply(["ok": true])
                    case .failure(let error): reply(self.failure(error.localizedDescription))
                    }
                }
                return
            case "catch-up":
                Task { @MainActor in
                    do { try await startCatchUp(); reply(["ok": true]) }
                    catch { reply(failure(error.localizedDescription)) }
                }
                return
            case "history": showHistory()
            case "vocabulary": openVocabulary()
            case "permissions": showAccessibilitySetup()
            default: reply(failure("Action unavailable")); return
            }
            reply(["ok": true]); return
        }
        reply(deckSnapshot())
    }

    func deckSnapshot() -> [String: Any] {
        lastDeckSnapshot = ProcessInfo.processInfo.systemUptime
        let capturing = session.phase == .listening || session.phase == .recording
        let dictation = waitingPaste != nil ? "Paste wait" : capturing && !(captureReady && capture.isReady)
            ? "Connecting" : session.phase == .idle ? overlay?.deckFeedback ?? "Ready" : session.phase.rawValue.capitalized
        let muted = conversation?["listeningPaused"] as? Bool
        let chat = conversation?["approval"] as? Bool == true ? "Approval" : muted == true ? "Muted" :
            conversation?["message"] as? String == "Paused" || pausedConversation != nil ? "Paused" :
            ["idle": "Voice off", "starting": "Starting", "listening": "Listening", "recording": "Recording",
             "decoding": "Transcribing", "thinking": "Thinking", "speaking": "Speaking"][conversation?["phase"] as? String ?? ""] ?? "Ready"
        let canStop = conversation != nil || speech.active || capturing || session.phase == .processing || waitingPaste != nil
        let playback = speech.playbackState == "idle" ? "Ready" : speech.playbackState.capitalized
        return ["ok": true, "dictation": dictation, "conversation": chat, "mute": muted.map { $0 ? "Muted" : "Live" } ?? "Unavailable",
               "interrupt": conversation?["interrupt"] as? Bool == true ? "Ready" : "Unavailable",
               "transcript": conversation == nil ? "Unavailable" : "Open", "new-chat": "Start",
               "stop": canStop ? "Stop" : "Unavailable", "auto": autoSettings.enabled ? "On" : "Off",
               "submit": autoSubmitEnabled ? "On" : "Off", "correction": correctionSettings.enabled ? "On" : "Off",
               "send": autoTarget != nil && session.phase == .recording ? "Send" : "Unavailable",
               "progress": overlay?.deckProgress ?? 0, "return": returnToField ? "On" : "Off", "pill": overlayEnabled ? "Shown" : "Hidden",
               "speech": speech.settings.enabled ? "On" : "Off", "play": playback,
               "catch-up": speech.isRecap ? playback : "Ready", "history": "Open", "vocabulary": "Open", "permissions": "Open",
               "level": min(5, Int(sqrt(max(0, conversation == nil ? capture.level : conversationLevel)) * 12))]
    }
}
