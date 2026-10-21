import AppKit
import TalkCore

final class SpeechController {
    let settings = SpeechSettings()
    private let reader = SpeechTargetReader()
    private let queue = DispatchQueue(label: "hey-talk.speech-target", qos: .userInitiated)
    private let engine = SupertonicSpeechEngine()
    private let output = NativeSpeechOutput()
    private lazy var playback = SpeechPlayback(engine: engine, output: output)
    private let overlay = SpeechOverlay()
    private let sourceHighlight = SpeechSourceHighlight()
    private var geometryPending = false
    private var lastGeometry: TimeInterval = 0
    private var readingLocation = 0
    private var scrollMonitor: Any?
    private var timer: Timer?
    private var notifications: [NSObjectProtocol] = []
    private var candidate: SpeechTarget?
    private var reading: SpeechTarget?
    private var pending = 0
    private var revision: UInt64 = 0
    private var lastPoint = CGPoint.zero
    private var lastMove: TimeInterval = 0
    private var lastRead: TimeInterval = 0
    private var lastHit: TimeInterval = 0
    private var release: DispatchWorkItem?
    private var sleeping = false
    private var warmedApp: pid_t?
    private var highlightTimer: Timer?
    var canOffer: () -> Bool = { true }
    var willPlay: () -> Bool = { true }
    var onActivityChanged: ((Bool) -> Void)?
    var onAllowedChanged: ((Bool) -> Void)?
    var onNotice: ((String) -> Void)?
    var active: Bool { playback.active }
    var status: [String: Any] {
        var result = settings.json
        result["state"] = playback.state.rawValue
        result["model"] = "sherpa-onnx-supertonic-3-tts-int8-2026-05-11"; result["model_ready"] = SpeechModel.available
        result["rate"] = playback.rate; result["overlay"] = overlay.visible
        result["first_audio_ms"] = playback.firstAudioMS
        result["worker_pid"] = engine.pid
        result["error"] = playback.error
        if let reading { result["source_app"] = reading.bundleID }
        // No captured text in routine status or logs.
        return result
    }
    func start() {
        scrollMonitor = NSEvent.addGlobalMonitorForEvents(matching: .scrollWheel) { [weak self] _ in
            guard let self else { return }; self.sourceHighlight.hide()
            if !self.active { self.revision &+= 1; self.candidate = nil; self.overlay.hide(); self.lastRead = 0 }
        }
        playback.onChange = { [weak self] in self?.changed() }
        output.onInterruption = { [weak self] in
            guard let self, self.active else { return }
            self.playback.fail("Audio output changed. Press Play to resume reading.")
        }
        overlay.onPlay = { [weak self] in self?.toggle(fromOverlay: true) }
        overlay.onStop = { [weak self] in self?.stop() }
        overlay.onSpeed = { [weak self] in
            guard let self else { return }
            let rates: [Float] = [0.75, 1, 1.25, 1.5, 2]
            self.playback.rate = rates.first(where: { $0 > self.playback.rate }) ?? rates[0]
        }
        let timer = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in self?.refresh() }
        timer.tolerance = 0.02; RunLoop.main.add(timer, forMode: .common); self.timer = timer
        let center = NSWorkspace.shared.notificationCenter
        notifications.append(center.addObserver(forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main) { [weak self] _ in self?.refresh() })
        for name in [NSWorkspace.willSleepNotification, NSWorkspace.sessionDidResignActiveNotification] {
            notifications.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in self?.sleeping = true; self?.stop() })
        }
        for name in [NSWorkspace.didWakeNotification, NSWorkspace.sessionDidBecomeActiveNotification] {
            notifications.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in self?.sleeping = false })
        }
    }
    func shutdown() {
        if let scrollMonitor { NSEvent.removeMonitor(scrollMonitor); self.scrollMonitor = nil }
        timer?.invalidate(); timer = nil
        notifications.forEach { NSWorkspace.shared.notificationCenter.removeObserver($0) }; notifications = []
        stop(); release?.cancel()
    }
    func stop() {
        revision &+= 1; candidate = nil; reading = nil
        release?.cancel(); playback.stop(); overlay.hide(); sourceHighlight.hide()
    }
    func toggleEnabled() {
        try? settings.command([settings.enabled ? "off" : "on"])
        settingsChanged()
        onNotice?(settings.enabled ? "Read aloud on · Hyper+S to play" : "Read aloud off")
    }
    func pause() { if active && playback.state != .paused { playback.togglePause() } }
    func settingsChanged() { stop(); refresh() }
    func toggle(fromOverlay: Bool = false, completion: @escaping (Result<Void, Error>) -> Void = { _ in }) {
        if active { playback.togglePause(); completion(.success(())); return }
        let anchor = (fromOverlay || overlay.containsPointer) ? candidate ?? reading : nil
        resolve(point: anchor?.point, preferSelection: true) { [weak self] result in
            guard let self else { return }
            switch result {
            case .failure(let error): self.onNotice?(error.localizedDescription); completion(.failure(error))
            case .success(let target):
                let firstText = target.paragraphs.first.map { (target.text as NSString).substring(with: $0.range) } ?? target.text
                let anchorText = anchor.map { value in value.paragraphs.first.map { (value.text as NSString).substring(with: $0.range) } ?? value.text }
                if let anchorText, anchorText != firstText && target.source != "selection" {
                    self.candidate = nil; self.overlay.hide()
                    completion(.failure(TalkError("The paragraph changed. Hover over it again."))); return
                }
                guard self.willPlay() else { completion(.failure(TalkError("Finish dictating before reading aloud."))); return }
                self.reading = target; self.candidate = target; self.readingLocation = 0; self.release?.cancel()
                do { try self.playback.start(target.text); completion(.success(())) }
                catch { self.onNotice?(error.localizedDescription); completion(.failure(error)) }
            }
        }
    }
    func target(completion: @escaping (Result<SpeechTarget, Error>) -> Void) {
        resolve(point: overlay.containsPointer ? candidate?.point : nil, preferSelection: true, completion: completion)
    }
    private func refresh() {
        let allowed = !sleeping && settings.bundleIDs.contains(NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "")
        onAllowedChanged?(allowed)
        guard !sleeping, let app = NSWorkspace.shared.frontmostApplication, settings.allows(app.bundleIdentifier ?? "") else {
            warmedApp = nil
            if active { stop() } else { candidate = nil; overlay.hide(); sourceHighlight.hide() }
            return
        }
        if warmedApp != app.processIdentifier {
            warmedApp = app.processIdentifier; engine.warm(); scheduleRelease()
        }
        if active { refreshSourceHighlight(); return }
        guard canOffer() else { candidate = nil; overlay.hide(); sourceHighlight.hide(); return }
        if overlay.containsPointer { lastHit = ProcessInfo.processInfo.systemUptime; return }
        let now = ProcessInfo.processInfo.systemUptime, point = Self.pointer
        if hypot(point.x - lastPoint.x, point.y - lastPoint.y) > 3 {
            lastMove = now; lastPoint = point
            if var target = candidate, target.frame.contains(point), overlay.visible {
                target.point = point; candidate = target
                overlay.show(target: target, state: .idle, rate: playback.rate, fraction: 0, error: nil)
            } else { sourceHighlight.hide() }
        }
        if now - lastHit > 0.8 { overlay.hide(); sourceHighlight.hide() }
        guard now - lastMove >= 0.15, now - lastRead >= 0.4, pending == 0 else { return }
        lastRead = now
        resolve(point: point, preferSelection: false) { [weak self] result in
            guard let self, !self.active, self.canOffer(), hypot(Self.pointer.x - point.x, Self.pointer.y - point.y) < 4 else { return }
            if case .success(let target) = result {
                self.candidate = target; self.lastHit = ProcessInfo.processInfo.systemUptime
                self.overlay.show(target: target, state: .idle, rate: self.playback.rate, fraction: 0, error: nil)
                if let viewport = target.viewport { self.sourceHighlight.show(frame: target.frame, viewport: viewport, reading: false) }
                else { self.sourceHighlight.hide() }
                self.engine.warm(); self.scheduleRelease()
            } else if ProcessInfo.processInfo.systemUptime - self.lastHit > 0.8 {
                self.candidate = nil; self.overlay.hide(); self.sourceHighlight.hide()
            }
        }
    }
    private static var pointer: CGPoint {
        let mouse = NSEvent.mouseLocation
        return CGPoint(x: mouse.x, y: (NSScreen.screens.first?.frame.maxY ?? 0) - mouse.y)
    }
    private func resolve(point: CGPoint?, preferSelection: Bool, completion: @escaping (Result<SpeechTarget, Error>) -> Void) {
        guard !sleeping, let app = NSWorkspace.shared.frontmostApplication,
              let bundleID = app.bundleIdentifier, settings.allows(bundleID) else {
            completion(.failure(TalkError("Speech is only available in allowed apps"))); return
        }
        // A deliberate Play can queue behind the in-flight hover read.
        guard pending < 2 else { completion(.failure(TalkError("Finding paragraph. Try again."))); return }
        pending += 1
        let pid = app.processIdentifier, point = point ?? Self.pointer, ticket = revision
        queue.async { [weak self] in
            guard let self else { return }
            let result = Result { try self.reader.read(pid: pid, bundleID: bundleID, point: point, preferSelection: preferSelection) }
            DispatchQueue.main.async {
                self.pending -= 1
                guard ticket == self.revision, !self.sleeping, self.settings.allows(bundleID),
                      NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else {
                    completion(.failure(TalkError("The target app changed"))); return
                }
                completion(result)
            }
        }
    }
    private func changed() {
        onActivityChanged?(active)
        if let target = reading ?? candidate {
            overlay.show(target: target, state: playback.state, rate: playback.rate, fraction: playback.progress, error: playback.error)
        }
        if !active {
            sourceHighlight.hide()
            if let error = playback.error { onNotice?(error) }
            scheduleRelease()
        }
        if playback.state == .playing, highlightTimer == nil {
            let timer = Timer(timeInterval: 0.04, repeats: true) { [weak self] _ in
                guard let self else { return }; self.overlay.highlight(self.playback.highlight)
            }
            timer.tolerance = 0.01; RunLoop.main.add(timer, forMode: .common); highlightTimer = timer
        } else if playback.state != .playing {
            highlightTimer?.invalidate(); highlightTimer = nil
        }
    }
    private func refreshSourceHighlight() {
        let now = ProcessInfo.processInfo.systemUptime
        guard !geometryPending, now - lastGeometry >= 0.25, let target = reading else { return }
        if let location = playback.highlight?.range.location { readingLocation = location }
        let location = readingLocation, ticket = revision
        geometryPending = true; lastGeometry = now
        queue.async { [weak self] in
            guard let self else { return }
            let geometry = self.reader.geometry(for: target, location: location)
            DispatchQueue.main.async {
                self.geometryPending = false
                guard self.revision == ticket, self.active, self.reading?.captureID == target.captureID,
                      NSWorkspace.shared.frontmostApplication?.processIdentifier == target.pid else { return }
                if let geometry { self.sourceHighlight.show(frame: geometry.frame, viewport: geometry.viewport, reading: true) }
                else { self.sourceHighlight.hide() }
            }
        }
    }
    private func scheduleRelease() {
        release?.cancel()
        let work = DispatchWorkItem { [weak self] in guard let self, !self.active else { return }; self.engine.cancel() }
        release = work; DispatchQueue.main.asyncAfter(deadline: .now() + 600, execute: work)
    }
}
