import AppKit
import QuartzCore
import TalkCore

private final class RecordingPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

private final class DockMotion: NSAnimation {
    private let panel: NSPanel
    private let startY: CGFloat
    let destination: CGFloat

    init(panel: NSPanel, destination: CGFloat) {
        self.panel = panel; startY = panel.frame.minY; self.destination = destination
        super.init(duration: 0.18, animationCurve: .easeOut)
        animationBlockingMode = .nonblocking
        frameRate = Float(panel.screen?.maximumFramesPerSecond ?? 60)
    }
    required init?(coder: NSCoder) { fatalError("DockMotion is not archived") }
    override var runLoopModesForAnimating: [RunLoop.Mode]? { [.common] }
    override var currentProgress: NSAnimation.Progress {
        didSet {
            // Only move vertically; recording and notices may resize the panel mid-flight.
            panel.setFrameOrigin(NSPoint(x: panel.frame.minX, y: startY + CGFloat(currentValue) * (destination - startY)))
        }
    }
}

private final class WaveView: NSView {
    static let interval: TimeInterval = 0.08
    private let bars = (0..<Waveform.count).map { _ in CALayer() }
    var color = TalkDesign.active { didSet { updateColors() } }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        for (index, bar) in bars.enumerated() {
            bar.position = CGPoint(x: index * 5 + 1, y: 12)
            bar.bounds = CGRect(x: 0, y: 0, width: 2, height: 4)
            bar.cornerRadius = 1
            layer?.addSublayer(bar)
        }
        updateColors()
    }
    required init?(coder: NSCoder) { fatalError("WaveView is not archived") }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); updateColors() }
    private func updateColors() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            for bar in bars { bar.backgroundColor = color.cgColor }
        }
    }
    func update(level: Double?) {
        CATransaction.begin()
        CATransaction.setDisableActions(level == nil || NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
        CATransaction.setAnimationDuration(Self.interval)
        CATransaction.setAnimationTimingFunction(CAMediaTimingFunction(name: .linear))
        for (i, bar) in bars.enumerated() {
            if let level {
                bar.bounds.size.height = i + 1 < bars.count ? bars[i + 1].bounds.height : Waveform.height(level)
            } else {
                bar.removeAnimation(forKey: "bounds")
                bar.bounds.size.height = 4
            }
        }
        CATransaction.commit()
    }
}

private final class IdentityView: NSView {
    var color = TalkDesign.accent { didSet { needsDisplay = true } }
    override func draw(_ dirtyRect: NSRect) {
        color.setStroke()
        let scale = bounds.width / 24
        for (x, height) in [(7.0, 6.0), (12.0, 14.0), (17.0, 9.0)] {
            let bar = NSBezierPath()
            bar.move(to: NSPoint(x: x * scale, y: (12 - height / 2) * scale))
            bar.line(to: NSPoint(x: x * scale, y: (12 + height / 2) * scale))
            bar.lineWidth = 2 * scale; bar.lineCapStyle = .round; bar.stroke()
        }
    }
}

private final class ClickSurface: GlassFace {
    var onClick: (() -> Void)?
    var onHover: ((Bool) -> Void)?
    var makeMenu: (() -> NSMenu)?
    var onDrag: (() -> Void)?
    private var start: NSPoint?
    private var origin: NSPoint?
    private(set) var dragged = false
    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.activeAlways, .mouseEnteredAndExited, .inVisibleRect], owner: self))
        super.updateTrackingAreas()
    }
    override func mouseEntered(with event: NSEvent) { onHover?(true) }
    override func mouseExited(with event: NSEvent) { onHover?(false) }
    override func mouseDown(with event: NSEvent) { start = NSEvent.mouseLocation; origin = window?.frame.origin; dragged = false }
    override func mouseDragged(with event: NSEvent) {
        guard let start, let origin else { return }
        let current = NSEvent.mouseLocation
        if hypot(current.x - start.x, current.y - start.y) > 3 { dragged = true }
        if dragged {
            window?.setFrameOrigin(NSPoint(x: origin.x + current.x - start.x, y: origin.y + current.y - start.y))
            onDrag?()
        }
    }
    override func mouseUp(with event: NSEvent) { if !dragged { onClick?() }; start = nil; dragged = false }
    override func rightMouseDown(with event: NSEvent) {
        guard let menu = menu(for: event) else { return }
        // This panel never activates the app, so AppKit can miss clicks in other apps.
        let outsideClick = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown, .otherMouseDown]) { [weak menu] _ in
            menu?.cancelTracking()
        }
        defer { if let outsideClick { NSEvent.removeMonitor(outsideClick) } }
        NSMenu.popUpContextMenu(menu, with: event, for: self)
    }
    override func menu(for event: NSEvent) -> NSMenu? { makeMenu?() }
    override func accessibilityPerformPress() -> Bool { onClick?(); return true }
}

final class Overlay {
    private let panel: RecordingPanel
    private let root = NSView(frame: NSRect(x: 0, y: 0, width: 200, height: 84))
    private let pill: NSView
    private let pillContent = ClickSurface()
    private let identity = IdentityView()
    private let wave = WaveView()
    private let resultIcon = NSImageView()
    private let confirmation = NSTextField(labelWithString: "")
    private let connecting = NSTextField(labelWithString: "Connecting…")
    private var waitingForAudio = false
    private var connectingDelay: DispatchWorkItem?
    private let autoHalo = CAShapeLayer()
    private var autoMode = false
    private let glow = CAShapeLayer()
    private let correctionIndicator = CALayer()
    private let submitIndicator = CALayer()
    private var correctionEnabled = true
    private var autoSubmitEnabled = true
    private var progress = TranscriptionProgress(audioSeconds: 0, correction: false)
    private var processingStarted: TimeInterval = 0
    private var completionDismiss: DispatchWorkItem?
    private let depth = CAShapeLayer()
    private let halo = CAShapeLayer()
    private var timer: Timer?
    private var dockTimer: Timer?
    private let dockQueue = DispatchQueue(label: "hey-talk.dock-geometry", qos: .utility)
    private var dockReadPending = false
    private var panelAnchorX: CGFloat = 100
    private var layoutRevision = 0
    private var dockList: AXUIElement?
    private var dockRestY: CGFloat?
    private var dockMotion: DockMotion?
    private var phase: Phase = .idle
    private var conversation: [String: Any]?
    private var expanded = false
    private var enabled = true
    private var pasteWait: PasteWaitView?
    var onCancelPaste: (() -> Void)?
    private var notice: OverlayNotice?
    private var status: OverlayNotice?
    private var closingStatus = false
    private var statusDismiss: DispatchWorkItem?
    private var noticeDismiss: DispatchWorkItem?
    private let noticeRecordButton = NSButton()
    var onToggle: (() -> Void)?
    var onOpenAccessibility: (() -> Void)?
    var onToggleSpeech: (() -> Void)?
    var onToggleAuto: (() -> Void)?
    var onToggleAutoSubmit: (() -> Void)?
    var onToggleReturnToField: (() -> Void)?
    var returnToFieldEnabled: () -> Bool = { false }
    var onToggleCorrection: (() -> Void)?
    var onShowHistory: (() -> Void)?
    var onChatWithContext: (() -> Void)?
    var onInterruptConversation: (() -> Void)?
    var onStopConversation: (() -> Void)?
    var speechEnabled: () -> Bool = { false }
    var level: () -> Double = { 0 }

    init() {
        panel = RecordingPanel(contentRect: root.frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.title = "Hey Talk — Recording"
        panel.level = .statusBar
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        panel.isOpaque = false; panel.backgroundColor = .clear
        panel.hasShadow = false; panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.contentView = root
        root.wantsLayer = true
        depth.fillColor = NSColor.black.withAlphaComponent(0.035).cgColor
        depth.shadowColor = NSColor.black.cgColor
        depth.shadowOpacity = 0.22; depth.shadowRadius = 6
        depth.shadowOffset = CGSize(width: 0, height: -3)
        root.layer?.addSublayer(depth)
        autoHalo.name = "auto-mode-glow"
        autoHalo.fillColor = nil
        autoHalo.strokeColor = TalkDesign.automatic.withAlphaComponent(0.65).cgColor
        autoHalo.lineWidth = 1.25
        autoHalo.shadowColor = TalkDesign.automatic.cgColor
        autoHalo.shadowOpacity = 0.65; autoHalo.shadowRadius = 6
        autoHalo.shadowOffset = .zero
        autoHalo.isHidden = true
        root.layer?.addSublayer(autoHalo)
        glow.name = "transcription-progress"
        glow.fillColor = nil; glow.strokeColor = TalkDesign.accent.cgColor
        glow.lineWidth = 1.5; glow.lineCap = .round
        glow.shadowColor = TalkDesign.accent.cgColor
        glow.shadowOpacity = 0.5; glow.shadowRadius = 3; glow.shadowOffset = .zero
        halo.fillColor = TalkDesign.accent.withAlphaComponent(0.12).cgColor
        halo.shadowColor = TalkDesign.accent.cgColor; halo.shadowOpacity = 0.3; halo.shadowRadius = 6
        halo.shadowOffset = .zero
        glow.isHidden = true; halo.isHidden = true
        root.layer?.addSublayer(halo)
        root.layer?.addSublayer(glow)
        if #available(macOS 26, *) {
            // This small floating control sits over other apps. Clear glass
            // preserves that backdrop instead of covering it with a frosted fill.
            let glass = NSGlassEffectView(); glass.style = .clear; glass.cornerRadius = 18
            glass.contentView = pillContent; pill = glass
            let container = NSGlassEffectContainerView(frame: root.bounds)
            container.autoresizingMask = [.width, .height]
            container.spacing = 8
            let content = NSView(frame: root.bounds)
            content.autoresizingMask = [.width, .height]
            container.contentView = content; root.addSubview(container)
            content.addSubview(pill)
        } else {
            func glass(_ content: NSView, radius: CGFloat) -> NSView {
                let view = NSVisualEffectView(); view.material = .popover
                view.blendingMode = .behindWindow; view.state = .active
                view.wantsLayer = true; view.layer?.cornerRadius = radius; view.layer?.masksToBounds = true
                content.autoresizingMask = [.width, .height]; view.addSubview(content)
                return view
            }
            pill = glass(pillContent, radius: 18)
            root.addSubview(pill)
        }
        pill.wantsLayer = true
        // Keep the live waveform inside the capsule while it expands.
        pill.clipsToBounds = true
        wave.isHidden = true
        wave.identifier = NSUserInterfaceItemIdentifier("recording-waveform")
        pillContent.addSubview(identity); pillContent.addSubview(wave)
        pillContent.wantsLayer = true
        for (indicator, name, symbol) in [(correctionIndicator, "ai-disabled", "brain"), (submitIndicator, "auto-submit", "return")] {
            indicator.name = name
            let configuration = NSImage.SymbolConfiguration(pointSize: 6, weight: .semibold)
                .applying(.init(paletteColors: [.systemRed]))
            let image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?.withSymbolConfiguration(configuration)
            indicator.contents = image?.cgImage(forProposedRect: nil, context: nil, hints: nil)
            indicator.bounds = NSRect(origin: .zero, size: image?.size ?? .zero)
            indicator.contentsGravity = .resizeAspect
            indicator.isHidden = true
            pillContent.layer?.addSublayer(indicator)
        }
        confirmation.font = .systemFont(ofSize: 12, weight: .medium)
        pillContent.addSubview(resultIcon)
        resultIcon.isHidden = true
        confirmation.textColor = .labelColor
        confirmation.isHidden = true
        pillContent.addSubview(confirmation)
        connecting.font = .systemFont(ofSize: 12, weight: .medium)
        connecting.textColor = .secondaryLabelColor
        connecting.isHidden = true
        pillContent.addSubview(connecting)
        noticeRecordButton.title = ""
        noticeRecordButton.isBordered = false
        noticeRecordButton.setAccessibilityLabel("Start dictation")
        noticeRecordButton.target = self
        noticeRecordButton.action = #selector(recordFromNotice)
        noticeRecordButton.isHidden = true
        pillContent.addSubview(noticeRecordButton)
        pillContent.setAccessibilityElement(true); pillContent.setAccessibilityRole(.button)
        pillContent.setAccessibilityLabel("Start dictation")
        updateModeAppearance()
        pillContent.onClick = { [weak self] in if self?.pasteWait == nil { self?.onToggle?() } }
        pillContent.makeMenu = { [weak self] in self?.contextMenu() ?? NSMenu() }
        pillContent.onHover = { [weak self] hover in self?.pillContent.emphasized = hover }
        pillContent.onDrag = { [weak self] in
            self?.dockMotion?.stop(); self?.dockMotion = nil; self?.dockRestY = nil
        }
        layout(animated: false)
        reposition()
        NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in self?.reposition() }
    }
    func reposition() {
        let screen = NSScreen.screens.first(where: { NSMouseInRect(NSEvent.mouseLocation, $0.frame, false) }) ?? NSScreen.main
        guard let screen else { return }
        let visible = screen.visibleFrame
        dockMotion?.stop(); dockMotion = nil; dockRestY = nil
        panel.setFrameOrigin(NSPoint(x: visible.midX - panelAnchorX, y: visible.minY + 12))
    }
    deinit {
        timer?.invalidate(); dockTimer?.invalidate(); connectingDelay?.cancel(); completionDismiss?.cancel(); dockMotion?.stop()
    }
    func show(_ visible: Bool) {
        enabled = visible
        if !visible {
            dockMotion?.stop(); dockMotion = nil
            clearStatus()
            clearNotice()
            updateAccessibilityLabel()
            layout(animated: false)
        }
        if visible { panel.orderFrontRegardless() } else { panel.orderOut(nil) }
        dockTimer?.invalidate(); dockTimer = nil
        if visible {
            let ticker = Timer(timeInterval: 0.1, repeats: true) { [weak self] timer in
                guard let self, !self.pillContent.dragged, !self.dockReadPending,
                      let top = NSScreen.screens.first?.frame.maxY else { return }
                self.dockReadPending = true
                self.dockQueue.async {
                    let frame = self.dockFrame(screenTop: top)
                    DispatchQueue.main.async {
                        self.dockReadPending = false
                        guard timer.isValid, !self.pillContent.dragged else { return }
                        self.avoidDock(frame)
                    }
                }
            }
            ticker.tolerance = 0.02
            RunLoop.main.add(ticker, forMode: .common); dockTimer = ticker
        }
        updateTimer()
    }
    private func dockFrame(screenTop top: CGFloat) -> NSRect? {
        guard AXIsProcessTrusted() else { return nil }
        if dockList == nil {
            guard let dock = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.dock").first else { return nil }
            let app = AXUIElementCreateApplication(dock.processIdentifier)
            AXUIElementSetMessagingTimeout(app, 0.05)
            var children: CFTypeRef?
            AXUIElementCopyAttributeValue(app, kAXChildrenAttribute as CFString, &children)
            dockList = (children as? [AXUIElement])?.first { child in
                var role: CFTypeRef?
                AXUIElementCopyAttributeValue(child, kAXRoleAttribute as CFString, &role)
                return role as? String == kAXListRole
            }
        }
        guard let dockList else { return nil }
        var position: CFTypeRef?, size: CFTypeRef?
        guard AXUIElementCopyAttributeValue(dockList, kAXPositionAttribute as CFString, &position) == .success,
              AXUIElementCopyAttributeValue(dockList, kAXSizeAttribute as CFString, &size) == .success,
              let position, let size,
              CFGetTypeID(position) == AXValueGetTypeID(), CFGetTypeID(size) == AXValueGetTypeID() else {
            self.dockList = nil; return nil
        }
        var point = CGPoint.zero, dimensions = CGSize.zero
        guard AXValueGetValue(position as! AXValue, .cgPoint, &point),
              AXValueGetValue(size as! AXValue, .cgSize, &dimensions) else { return nil }
        return NSRect(x: point.x, y: top - point.y - dimensions.height, width: dimensions.width, height: dimensions.height)
    }
    func avoidDock(_ dock: NSRect?) {
        let frame = panel.frame
        let restingY = dockRestY ?? frame.minY
        var destination = restingY
        if let dock, let screen = panel.screen?.frame,
           dock.width > dock.height, dock.intersects(screen),
           frame.maxX > dock.minX, frame.minX < dock.maxX {
            destination = max(restingY, dock.maxY + 12)
        }
        if let motion = dockMotion, motion.destination == destination {
            if !motion.isAnimating && destination == restingY { dockMotion = nil; dockRestY = nil }
            return
        }
        dockMotion?.stop(); dockMotion = nil
        dockRestY = restingY
        if !panel.isVisible || NSWorkspace.shared.accessibilityDisplayShouldReduceMotion || destination == frame.minY {
            if frame.minY != destination { panel.setFrameOrigin(NSPoint(x: frame.minX, y: destination)) }
            if destination == restingY { dockRestY = nil }
        } else {
            let motion = DockMotion(panel: panel, destination: destination)
            dockMotion = motion
            motion.start()
        }
    }
    func update(_ next: Phase) {
        guard conversation == nil else { return }
        update(next, waitingForAudio: false)
    }
    func showConversation(_ state: [String: Any]?) {
        conversation = state
        guard let state else { update(.idle); return }
        let name = state["phase"] as? String
        let busy = ["decoding", "thinking", "starting"].contains(name) && state["approval"] as? Bool != true
        let next: Phase = name == "recording" ? .recording : busy ? .processing : .idle
        clearStatus(); clearNotice()
        update(next, waitingForAudio: false)
        connecting.stringValue = state["message"] as? String ?? "Ready"
        connecting.isHidden = false
        correctionIndicator.isHidden = true; submitIndicator.isHidden = true
        glow.isHidden = true
        updateModeAppearance(); updateAccessibilityLabel(); layout(animated: true)
        panel.displayIfNeeded()
    }
    private func update(_ next: Phase, waitingForAudio: Bool) {
        pasteWait?.removeFromSuperview(); pasteWait = nil
        identity.isHidden = false
        correctionIndicator.isHidden = correctionEnabled; submitIndicator.isHidden = autoSubmitEnabled
        completionDismiss?.cancel(); completionDismiss = nil
        connectingDelay?.cancel(); connectingDelay = nil
        if confirmation.isHidden || next == .recording { clearNotice() }
        self.waitingForAudio = waitingForAudio
        connecting.isHidden = true
        phase = next; expanded = next == .recording
        updateModeAppearance()
        wave.update(level: nil)
        wave.isHidden = next != .recording
        wave.alphaValue = waitingForAudio ? 0.35 : 1
        identity.layer?.removeAllAnimations()
        glow.removeAllAnimations(); halo.removeAllAnimations()
        glow.isHidden = next != .processing; halo.isHidden = next != .processing && !waitingForAudio
        if next == .processing { processingStarted = ProcessInfo.processInfo.systemUptime; glow.strokeEnd = 0.02 }
        updateAccessibilityLabel()
        layout(animated: true); updateTimer()
        if next != .processing { glow.removeAllAnimations(); halo.removeAllAnimations() }
        if next == .processing || waitingForAudio {
            if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
                let pulse = CABasicAnimation(keyPath: "opacity")
                pulse.fromValue = 0.55; pulse.toValue = 1; pulse.duration = 0.95
                pulse.autoreverses = true; pulse.repeatCount = .infinity
                halo.add(pulse, forKey: "breathe")
            }
        }
    }
    func processing(audioSeconds: Double, correction: Bool) {
        progress = TranscriptionProgress(audioSeconds: audioSeconds, correction: correction)
        guard phase != .processing else { return }
        update(.processing)
    }
    func finishProcessing() {
        update(.idle)
        glow.isHidden = false; halo.isHidden = false
        CATransaction.begin(); CATransaction.setAnimationDuration(0.12)
        glow.strokeEnd = 1
        CATransaction.commit()
        let dismiss = DispatchWorkItem { [weak self] in
            self?.glow.isHidden = true; self?.halo.isHidden = true
            self?.completionDismiss = nil
        }
        completionDismiss = dismiss
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.18, execute: dismiss)
    }
    func waitToPaste(_ instruction: String, while retry: Timer) {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak self] in
            guard retry.isValid else { return }
            self?.showPasteWait(instruction)
        }
    }
    private func showPasteWait(_ instruction: String) {
        update(.idle); clearNotice(); clearStatus()
        let view = PasteWaitView(instruction: instruction) { [weak self] in self?.onCancelPaste?() }
        pasteWait = view; pillContent.addSubview(view)
        identity.isHidden = true; correctionIndicator.isHidden = true; submitIndicator.isHidden = true
        updateAccessibilityLabel(); layout(animated: true)
    }
    func preparing(_ phase: Phase = .recording) {
        guard conversation == nil, !waitingForAudio else { return }
        update(phase, waitingForAudio: true)
        connecting.stringValue = "Connecting…"
        let delay = DispatchWorkItem { [weak self] in
            guard let self, self.waitingForAudio else { return }
            self.connecting.isHidden = false
            self.layout(animated: true)
        }
        connectingDelay = delay
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.7, execute: delay)
        panel.displayIfNeeded()
    }
    func setAutoMode(_ enabled: Bool) {
        autoMode = enabled
        updateModeAppearance(); updateAccessibilityLabel(); layout(animated: true)
    }
    func setCorrection(_ enabled: Bool) {
        correctionEnabled = enabled
        correctionIndicator.isHidden = enabled || pasteWait != nil || conversation != nil
        updateModeAppearance(); updateAccessibilityLabel(); layout(animated: true)
    }
    func setAutoSubmit(_ enabled: Bool) {
        guard autoSubmitEnabled != enabled else { return }
        autoSubmitEnabled = enabled
        submitIndicator.isHidden = enabled || pasteWait != nil || conversation != nil
        updateModeAppearance(); updateAccessibilityLabel(); layout(animated: true)
    }
    func contextMenu() -> NSMenu {
        let menu = NSMenu()
        if let conversation {
            let action = menu.addItem(withTitle: conversation["action"] as? String ?? "Start voice", action: #selector(recordFromNotice), keyEquivalent: "")
            action.target = self
            if conversation["interrupt"] as? Bool == true {
                menu.addItem(withTitle: "Listen for next message", action: #selector(interruptConversation), keyEquivalent: "").target = self
            }
            if conversation["phase"] as? String == "recording" {
                menu.addItem(withTitle: "Cancel recording", action: #selector(stopConversation), keyEquivalent: "").target = self
            }
            return menu
        }
        let history = menu.addItem(withTitle: "Dictation History…", action: #selector(showHistory), keyEquivalent: "")
        history.target = self
        for (section, items) in [
            ("Dictation", [
                ("AI Correction", #selector(toggleCorrection), correctionEnabled, ""),
                ("Auto Submit", #selector(toggleAutoSubmit), autoSubmitEnabled, "e"),
                ("Auto Dictation", #selector(toggleAutoMode), autoMode, "v"),
                ("Return to Field Before Pasting", #selector(toggleReturnToField), returnToFieldEnabled(), "")
            ]),
            ("Speech", [("Read Aloud", #selector(toggleSpeechMode), speechEnabled(), "t")])
        ] {
            menu.addItem(.separator())
            menu.addItem(.sectionHeader(title: section))
            for (title, action, enabled, key) in items {
                let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
                item.keyEquivalentModifierMask = [.control, .command, .option, .shift]
                item.target = self; item.state = enabled ? .on : .off; menu.addItem(item)
            }
        }
        menu.addItem(.separator())
        menu.addItem(.sectionHeader(title: "Setup"))
        let context = NSMenuItem(title: "Chat with Context", action: #selector(chatWithContext), keyEquivalent: "m")
        context.keyEquivalentModifierMask = [.control, .command, .option, .shift]
        context.target = self; menu.addItem(context)
        let item = NSMenuItem(title: "Paste Permissions…", action: #selector(openAccessibility), keyEquivalent: "")
        item.target = self; menu.addItem(item)
        return menu
    }
    @objc private func openAccessibility() { onOpenAccessibility?() }
    @objc private func toggleSpeechMode() { onToggleSpeech?() }
    @objc private func toggleAutoMode() { onToggleAuto?() }
    @objc private func toggleAutoSubmit() { onToggleAutoSubmit?() }
    @objc private func toggleReturnToField() { onToggleReturnToField?() }
    @objc private func toggleCorrection() { onToggleCorrection?() }
    @objc private func showHistory() { onShowHistory?() }
    @objc private func chatWithContext() { onChatWithContext?() }
    @objc private func interruptConversation() { onInterruptConversation?() }
    @objc private func stopConversation() { onStopConversation?() }
    private func updateModeAppearance() {
        if let conversation {
            let recording = ["listening", "recording"].contains(conversation["phase"] as? String)
            let color = conversation["approval"] as? Bool == true ? NSColor.systemOrange : recording ? TalkDesign.active : TalkDesign.accent
            pillContent.accent = color; identity.color = color; wave.color = color
            autoHalo.isHidden = true
            pillContent.toolTip = (conversation["detail"] as? String ?? "") + " · " + (conversation["action"] as? String ?? "") + " · Hyper+R"
            return
        }
        let tint = resultIcon.isHidden ? nil : resultIcon.contentTintColor
        pillContent.accent = tint ?? (autoMode ? TalkDesign.automatic : phase == .recording ? TalkDesign.active : TalkDesign.accent)
        let rim = tint ?? TalkDesign.automatic
        CATransaction.begin()
        CATransaction.setDisableActions(NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
        CATransaction.setAnimationDuration(0.2)
        autoHalo.strokeColor = rim.withAlphaComponent(tint == nil ? 0.65 : 0.42).cgColor
        autoHalo.shadowColor = rim.cgColor
        autoHalo.isHidden = !autoMode && tint == nil
        CATransaction.commit()
        identity.color = autoMode ? TalkDesign.automatic : TalkDesign.accent
        wave.color = autoMode ? TalkDesign.automatic : TalkDesign.active
        pillContent.toolTip = autoMode
            ? "Auto · Speak to record · Pause to submit · Enter to send now · Esc to cancel · Hyper+V to turn off · \(DictationShortcut.help)"
            : "\(DictationShortcut.help) · Hyper+V: Auto mode · Click to start / stop · Drag to move · Right-click for modes"
        pillContent.toolTip = "AI correction \(correctionEnabled ? "on" : "off") · Auto Submit \(autoSubmitEnabled ? "on" : "off") · " + (pillContent.toolTip ?? "")
    }
    private func updateAccessibilityLabel() {
        if let conversation {
            pillContent.setAccessibilityRole(.button)
            pillContent.setAccessibilityLabel("Chat · " + (conversation["message"] as? String ?? "Ready") + " · " + (conversation["action"] as? String ?? "Start voice"))
            return
        }
        if let pasteWait {
            pillContent.setAccessibilityRole(.group)
            pillContent.setAccessibilityLabel(pasteWait.accessibilityLabel())
            return
        }
        let action = waitingForAudio ? "Opening microphone" : phase == .listening ? "Listening · Speak to record" : phase == .recording ? "Stop dictation" : phase == .processing ? "Transcribing" : "Start dictation"
        pillContent.setAccessibilityLabel((autoMode ? "Auto mode · " : "") + action + " · AI correction \(correctionEnabled ? "on" : "off") · Auto Submit \(autoSubmitEnabled ? "on" : "off")")
    }
    private func layout(animated: Bool) {
        let duration = phase == .recording ? 0.16 : 0.3
        if #available(macOS 26, *), let glass = pill as? NSGlassEffectView {
            let style: NSGlassEffectView.Style = pasteWait != nil || notice != nil || status != nil || !confirmation.isHidden ? .regular : .clear
            if glass.style != style { glass.style = style }
        }
        let showConnecting = !connecting.isHidden
        let statusWidth: CGFloat = conversation != nil && !expanded ? 144 : 200
        let leftExtension = confirmation.isHidden ? 0 : ceil(confirmation.frame.width) + 26
        let rightExtension = closingStatus ? 0 : status?.frame.width ?? 0
        let size = pasteWait?.frame.size ?? notice.map { NSSize(width: max($0.frame.width, showConnecting ? 200 : 104), height: $0.frame.height + 36) }
            ?? (showConnecting ? NSSize(width: statusWidth, height: 36) : expanded ? TalkDesign.recordingSize : TalkDesign.idleSize)
        // Status messages grow only the right edge of the window, keeping the recording control still.
        let baseWidth = max(200, size.width + 96, size.width + 2 * leftExtension + 16)
        let anchorX = baseWidth / 2
        let target = NSSize(width: max(baseWidth, anchorX + size.width / 2 + rightExtension + 8), height: size.height + 48)
        layoutRevision += 1
        let revision = layoutRevision
        let shrinking = animated && panel.isVisible && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion &&
            (target.width < panel.frame.width || target.height < panel.frame.height)
        if shrinking {
            // Keep closing glass inside the transparent window until its last frame.
            resizePanel(to: NSSize(width: max(target.width, panel.frame.width), height: max(target.height, panel.frame.height)), anchorX: max(panelAnchorX, anchorX))
        } else { resizePanel(to: target, anchorX: anchorX) }
        let width = size.width + leftExtension + rightExtension, height = size.height
        let center = panelAnchorX + (rightExtension - leftExtension) / 2
        let frame = NSRect(x: center - width / 2, y: 12, width: width, height: height)
        // Clockwise from the top center, just outside the existing glass edge.
        let rim = CGMutablePath()
        let left = frame.minX - 1, right = frame.maxX + 1, bottom = frame.minY - 1, top = frame.maxY + 1
        let radius: CGFloat = 19
        rim.move(to: CGPoint(x: center, y: top))
        rim.addLine(to: CGPoint(x: right - radius, y: top))
        rim.addArc(tangent1End: CGPoint(x: right, y: top), tangent2End: CGPoint(x: right, y: bottom), radius: radius)
        rim.addArc(tangent1End: CGPoint(x: right, y: bottom), tangent2End: CGPoint(x: left, y: bottom), radius: radius)
        rim.addArc(tangent1End: CGPoint(x: left, y: bottom), tangent2End: CGPoint(x: left, y: top), radius: radius)
        rim.addArc(tangent1End: CGPoint(x: left, y: top), tangent2End: CGPoint(x: right, y: top), radius: radius)
        rim.closeSubpath()
        glow.path = rim
        halo.frame = CGRect(x: center - 32, y: 8, width: 64, height: 44)
        halo.path = CGPath(roundedRect: CGRect(x: 4, y: 4, width: 56, height: 36), cornerWidth: 18, cornerHeight: 18, transform: nil)
        if conversation != nil {
            halo.frame = frame
            halo.path = CGPath(roundedRect: CGRect(origin: .zero, size: frame.size), cornerWidth: 18, cornerHeight: 18, transform: nil)
        }
        let outline = CGPath(roundedRect: frame, cornerWidth: 18, cornerHeight: 18, transform: nil)
        let autoOutline = CGPath(roundedRect: frame.insetBy(dx: -0.5, dy: -0.5), cornerWidth: 18.5, cornerHeight: 18.5, transform: nil)
        if animated && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            for key in ["path", "shadowPath"] {
                let transition = CABasicAnimation(keyPath: key)
                transition.fromValue = depth.presentation()?.value(forKey: key) ?? depth.value(forKey: key)
                transition.toValue = outline; transition.duration = duration
                transition.timingFunction = CAMediaTimingFunction(controlPoints: 0.2, 0, 0, 1)
                depth.add(transition, forKey: key)
            }
            let transition = CABasicAnimation(keyPath: "path")
            transition.fromValue = autoHalo.presentation()?.path ?? autoHalo.path
            transition.toValue = autoOutline; transition.duration = duration
            transition.timingFunction = CAMediaTimingFunction(controlPoints: 0.2, 0, 0, 1)
            autoHalo.add(transition, forKey: "path")
        }
        CATransaction.begin(); CATransaction.setDisableActions(true)
        depth.path = outline; depth.shadowPath = outline
        autoHalo.path = autoOutline
        CATransaction.commit()
        let minimumLabelX: CGFloat = expanded ? 102 : 40
        let labelSize = connecting.intrinsicContentSize
        let labelWidth = min(ceil(labelSize.width), statusWidth - minimumLabelX - 8)
        let compactVoice = conversation != nil && showConnecting && !expanded
        let labelX = compactVoice ? max(minimumLabelX, (statusWidth - labelWidth) / 2) : minimumLabelX
        let contentOrigin = leftExtension + (size.width - (showConnecting ? statusWidth : TalkDesign.recordingSize.width)) / 2
        let markX = expanded || showConnecting ? contentOrigin + (compactVoice ? 16 : 9) : leftExtension + (size.width - 22) / 2
        NSAnimationContext.runAnimationGroup { context in
            context.duration = animated && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? duration : 0
            context.timingFunction = CAMediaTimingFunction(controlPoints: 0.2, 0, 0, 1)
            pill.animator().frame = frame
            pillContent.animator().frame = NSRect(origin: .zero, size: frame.size)
            status?.animator().alphaValue = closingStatus ? 0 : 1
            identity.animator().frame = NSRect(x: markX, y: 7, width: 22, height: 22)
            wave.animator().frame = NSRect(x: contentOrigin + 39, y: 6, width: 52, height: 24)
            let labelY = (18 + connecting.firstBaselineOffsetFromTop - labelSize.height - connecting.font!.capHeight / 2).rounded()
            connecting.animator().frame = NSRect(x: contentOrigin + labelX, y: labelY, width: labelWidth, height: labelSize.height)
        } completionHandler: { [weak self] in
            guard let self, layoutRevision == revision else { return }
            if closingStatus { clearStatus(); layout(animated: false) }
            else if shrinking { layout(animated: false) }
        }
        CATransaction.begin(); CATransaction.setDisableActions(true)
        correctionIndicator.position = CGPoint(x: markX + 23, y: 26)
        submitIndicator.position = CGPoint(x: markX + 23, y: 10)
        CATransaction.commit()
        if let notice { notice.frame.origin = NSPoint(x: (width - notice.frame.width) / 2, y: 36) }
        status?.frame.origin = NSPoint(x: leftExtension + size.width, y: 0)
        noticeRecordButton.isHidden = notice == nil && status == nil
        pillContent.setAccessibilityRole(pasteWait != nil || !noticeRecordButton.isHidden ? .group : .button)
        noticeRecordButton.frame = NSRect(x: notice == nil ? markX - 7 : (width - 36) / 2, y: 0, width: 36, height: 36)
        noticeRecordButton.setAccessibilityLabel(conversation?["action"] as? String ?? (phase == .recording ? "Stop dictation" : "Start dictation"))
    }
    private func resizePanel(to windowSize: NSSize, anchorX: CGFloat) {
        guard panel.frame.size != windowSize || panelAnchorX != anchorX else { return }
        let oldX = panel.frame.minX
        var frame = NSRect(x: oldX + panelAnchorX - anchorX, y: panel.frame.minY, width: windowSize.width, height: windowSize.height)
        if let screen = panel.screen {
            let visible = screen.visibleFrame
            frame.origin.x = min(max(frame.minX, visible.minX), visible.maxX - frame.width)
            frame.origin.y = min(max(frame.minY, visible.minY), visible.maxY - frame.height)
        }
        panel.setFrame(frame, display: false)
        panelAnchorX = anchorX
        let delta = oldX - panel.frame.minX
        guard delta != 0 else { return }
        pill.frame.origin.x += delta
        var transform = CGAffineTransform(translationX: delta, y: 0)
        depth.removeAllAnimations()
        depth.path = depth.path?.copy(using: &transform)
        depth.shadowPath = depth.path
        autoHalo.removeAllAnimations()
        autoHalo.path = autoHalo.path?.copy(using: &transform)
    }
    private func updateTimer() {
        timer?.invalidate(); timer = nil
        if phase == .processing, enabled {
            guard conversation == nil else { return }
            let ticker = Timer(timeInterval: 0.05, repeats: true) { [weak self] _ in
                guard let self else { return }
                CATransaction.begin()
                CATransaction.setDisableActions(NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
                CATransaction.setAnimationDuration(0.1)
                self.glow.strokeEnd = max(self.glow.strokeEnd, self.progress.fraction(elapsed: ProcessInfo.processInfo.systemUptime - self.processingStarted))
                CATransaction.commit()
            }
            ticker.tolerance = 0.01
            RunLoop.main.add(ticker, forMode: .common); timer = ticker
            return
        }
        guard phase == .recording, !waitingForAudio, enabled else { return }
        let ticker = Timer(timeInterval: WaveView.interval, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.wave.update(level: self.level())
        }
        ticker.tolerance = 0.01
        RunLoop.main.add(ticker, forMode: .common); timer = ticker
    }
    @objc private func recordFromNotice() { onToggle?() }
    private func clearNotice() {
        noticeDismiss?.cancel(); noticeDismiss = nil
        if !confirmation.isHidden {
            confirmation.isHidden = true; resultIcon.isHidden = true
            updateModeAppearance()
        }
        notice?.removeFromSuperview(); notice = nil
        noticeRecordButton.isHidden = true
        pillContent.setAccessibilityRole(.button)
    }
    private func dismissNotice() {
        guard notice != nil || !confirmation.isHidden else { return }
        clearNotice()
        updateAccessibilityLabel()
        layout(animated: true)
    }
    func showStatus(_ message: String) {
        guard enabled, pasteWait == nil, notice == nil else { return }
        clearStatus()
        let dismiss = { [weak self] in
            guard let self, !closingStatus else { return }
            if panel.isVisible && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
                closingStatus = true
            } else { clearStatus() }
            layout(animated: true)
        }
        let content = OverlayNotice(message: message, dismiss: dismiss, compact: true)
        if panel.isVisible && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion { content.alphaValue = 0 }
        status = content; pillContent.addSubview(content)
        layout(animated: true)
        let timer = DispatchWorkItem(block: dismiss)
        statusDismiss = timer
        DispatchQueue.main.asyncAfter(deadline: .now() + 3, execute: timer)
    }
    private func clearStatus() {
        statusDismiss?.cancel(); statusDismiss = nil
        status?.removeFromSuperview(); status = nil; closingStatus = false
    }
    func confirmAction(_ action: CorrectionAction, failed: Bool = false) {
        guard enabled, pasteWait == nil, notice == nil else { return }
        let message: String
        switch action {
        case .slack(.users): message = "People"
        case .slack(.chat): message = "Chat opened"
        case .slack(.channel): message = "Channel opened"
        case .pinThread(let pinned): message = failed ? (pinned ? "Couldn’t pin" : "Couldn’t unpin") : (pinned ? "Pinned" : "Unpinned")
        case .settleThread: message = failed ? "Couldn’t settle" : "Settled"
        case .rename: message = failed ? "Couldn’t rename" : "Renamed"
        case .newThread: message = failed ? "Couldn’t open thread" : "New thread"
        case .nextThread: message = failed ? "Couldn’t go to next chat" : "Next chat"
        case .switchThread(let position): message = failed ? "Couldn’t switch chats" : "Chat \(position)"
        case .listThreads where failed: message = "Couldn’t read chats"
        case .paste, .clearInput, .listThreads: return
        }
        showActivity(message, failed: failed)
    }
    func showActivity(_ message: String, failed: Bool) {
        guard enabled, pasteWait == nil, notice == nil else { return }
        clearNotice()
        confirmation.stringValue = message
        confirmation.sizeToFit()
        resultIcon.image = NSImage(systemSymbolName: failed ? "xmark" : "checkmark", accessibilityDescription: nil)
        resultIcon.symbolConfiguration = .init(pointSize: 11, weight: .semibold)
        resultIcon.contentTintColor = failed ? TalkDesign.failure : TalkDesign.success
        resultIcon.frame = NSRect(x: 12, y: 11.5, width: 13, height: 13)
        let fade = !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        resultIcon.alphaValue = fade ? 0 : 1; confirmation.alphaValue = resultIcon.alphaValue
        resultIcon.isHidden = false
        confirmation.setFrameOrigin(NSPoint(x: 29, y: (18 + confirmation.firstBaselineOffsetFromTop - confirmation.frame.height - confirmation.font!.capHeight / 2).rounded()))
        confirmation.isHidden = false
        updateModeAppearance()
        NSAnimationContext.runAnimationGroup { context in
            context.duration = fade ? 0.24 : 0
            resultIcon.animator().alphaValue = 1; confirmation.animator().alphaValue = 1
        }
        NSAccessibility.post(element: panel, notification: .announcementRequested, userInfo: [.announcement: message, .priority: NSAccessibilityPriorityLevel.low.rawValue])
        layout(animated: true)
        let dismiss = DispatchWorkItem { [weak self] in self?.dismissNotice() }
        noticeDismiss = dismiss
        DispatchQueue.main.asyncAfter(deadline: .now() + (failed ? 5 : 3), execute: dismiss)
    }
    func error(_ message: String, accessibilityAction: Bool = false, retry: (() -> Void)? = nil) {
        guard enabled, pasteWait == nil else { return }
        clearNotice()
        clearStatus()
        let action: (String, () -> Void)?
        if accessibilityAction && retry == nil { action = ("Enable Accessibility", { [weak self] in self?.onOpenAccessibility?() }) }
        else { action = nil }
        let content = OverlayNotice(message: message, action: action, retry: retry, dismiss: { [weak self] in self?.dismissNotice() })
        notice = content
        pillContent.addSubview(content)
        noticeRecordButton.isHidden = false
        pillContent.setAccessibilityRole(.group)
        pillContent.setAccessibilityLabel("Hey Talk")
        content.alphaValue = 0
        layout(animated: true)
        NSAnimationContext.runAnimationGroup { context in
            context.duration = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0 : 0.2
            content.animator().alphaValue = 1
        }
        // Keep the repair link available until clicked, dismissed, or recording resumes.
        if action == nil && retry == nil {
            let dismiss = DispatchWorkItem { [weak self] in self?.dismissNotice() }
            noticeDismiss = dismiss
            DispatchQueue.main.asyncAfter(deadline: .now() + 6, execute: dismiss)
        }
    }
}
