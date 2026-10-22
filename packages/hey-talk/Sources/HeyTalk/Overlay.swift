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
        frameRate = 60
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
    var color = TalkDesign.active { didSet { needsDisplay = true } }
    var heights = Array(repeating: 4.0, count: Waveform.count) { didSet { needsDisplay = true } }
    override func draw(_ dirtyRect: NSRect) {
        color.setFill()
        for (index, height) in heights.enumerated() {
            NSBezierPath(roundedRect: NSRect(x: CGFloat(index * 5), y: (bounds.height - height) / 2, width: 2, height: height), xRadius: 1, yRadius: 1).fill()
        }
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
    private let connecting = NSTextField(labelWithString: "Connecting…")
    private var waitingForAudio = false
    private var connectingDelay: DispatchWorkItem?
    private let autoHalo = CAShapeLayer()
    private var autoMode = false
    private let glow = CAGradientLayer()
    private let depth = CAShapeLayer()
    private let halo = CAShapeLayer()
    private var timer: Timer?
    private var dockTimer: Timer?
    private var dockList: AXUIElement?
    private var dockRestY: CGFloat?
    private var dockMotion: DockMotion?
    private var phase: Phase = .idle
    private var expanded = false
    private var enabled = true
    private var notice: OverlayNotice?
    private var modeNotice = false
    private var noticeDismiss: DispatchWorkItem?
    private let noticeRecordButton = NSButton()
    var onToggle: (() -> Void)?
    var onOpenAccessibility: (() -> Void)?
    var onToggleSpeech: (() -> Void)?
    var onToggleAuto: (() -> Void)?
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
        glow.type = .conic
        glow.startPoint = CGPoint(x: 0.5, y: 0.5); glow.endPoint = CGPoint(x: 0.5, y: 0)
        glow.colors = [TalkDesign.accent.withAlphaComponent(0.15).cgColor, TalkDesign.accent.withAlphaComponent(0.3).cgColor, TalkDesign.accent.cgColor, TalkDesign.active.cgColor, NSColor.white.cgColor]
        glow.locations = [0, 0.45, 0.75, 0.94, 1]
        let mask = CAShapeLayer()
        mask.path = CGPath(roundedRect: CGRect(x: 1, y: 1, width: 58, height: 38), cornerWidth: 19, cornerHeight: 19, transform: nil)
        mask.fillColor = nil; mask.strokeColor = NSColor.white.cgColor; mask.lineWidth = 1.5
        glow.mask = mask
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
        pillContent.onClick = { [weak self] in self?.onToggle?() }
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
        panel.setFrameOrigin(NSPoint(x: visible.midX - panel.frame.width / 2, y: visible.minY + 12))
    }
    deinit {
        timer?.invalidate(); dockTimer?.invalidate(); connectingDelay?.cancel(); dockMotion?.stop()
    }
    func show(_ visible: Bool) {
        enabled = visible
        if !visible {
            dockMotion?.stop(); dockMotion = nil
            dismissNotice(animated: false)
        }
        if visible { panel.orderFrontRegardless() } else { panel.orderOut(nil) }
        dockTimer?.invalidate(); dockTimer = nil
        if visible {
            avoidDock(dockFrame())
            let ticker = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in
                guard let self, !self.pillContent.dragged else { return }
                self.avoidDock(self.dockFrame())
            }
            ticker.tolerance = 0.02
            RunLoop.main.add(ticker, forMode: .common); dockTimer = ticker
        }
        updateTimer()
    }
    private func dockFrame() -> NSRect? {
        guard AXIsProcessTrusted(), let top = NSScreen.screens.first?.frame.maxY else { return nil }
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
            panel.setFrameOrigin(NSPoint(x: frame.minX, y: destination))
            if destination == restingY { dockRestY = nil }
        } else {
            let motion = DockMotion(panel: panel, destination: destination)
            dockMotion = motion
            motion.start()
        }
    }
    func update(_ next: Phase) {
        update(next, waitingForAudio: false)
    }
    private func update(_ next: Phase, waitingForAudio: Bool) {
        connectingDelay?.cancel(); connectingDelay = nil
        if !modeNotice { clearNotice() }
        self.waitingForAudio = waitingForAudio
        connecting.isHidden = true
        phase = next; expanded = next == .recording
        updateModeAppearance()
        wave.heights = Array(repeating: 4, count: Waveform.count)
        wave.isHidden = next != .recording
        wave.alphaValue = waitingForAudio ? 0.35 : 1
        identity.layer?.removeAllAnimations()
        glow.removeAllAnimations(); halo.removeAllAnimations()
        glow.isHidden = next != .processing; halo.isHidden = next != .processing
        updateAccessibilityLabel()
        layout(animated: true); updateTimer()
        if next == .processing {
            if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
                let shimmer = CABasicAnimation(keyPath: "opacity")
                shimmer.fromValue = 0.35; shimmer.toValue = 1; shimmer.duration = 0.9
                shimmer.autoreverses = true; shimmer.repeatCount = .infinity
                glow.add(shimmer, forKey: "shimmer")
                let pulse = CABasicAnimation(keyPath: "opacity")
                pulse.fromValue = 0.55; pulse.toValue = 1; pulse.duration = 0.95
                pulse.autoreverses = true; pulse.repeatCount = .infinity
                halo.add(pulse, forKey: "breathe")
            }
        }
    }
    func preparing() {
        guard !waitingForAudio else { return }
        update(.recording, waitingForAudio: true)
        let delay = DispatchWorkItem { [weak self] in
            guard let self, self.waitingForAudio else { return }
            self.connecting.isHidden = false
            self.layout(animated: true)
        }
        connectingDelay = delay
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.7, execute: delay)
    }
    func setAutoMode(_ enabled: Bool) {
        autoMode = enabled
        autoHalo.isHidden = !enabled
        updateModeAppearance(); updateAccessibilityLabel(); layout(animated: true)
    }
    func contextMenu() -> NSMenu {
        let menu = NSMenu()
        for (title, action, enabled) in [
            ("Read Aloud · Hyper+T", #selector(toggleSpeechMode), speechEnabled()),
            ("Auto Dictation · Hyper+V", #selector(toggleAutoMode), autoMode)
        ] {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
            item.target = self; item.state = enabled ? .on : .off; menu.addItem(item)
        }
        menu.addItem(.separator())
        let item = NSMenuItem(title: "Automatic Paste…", action: #selector(openAccessibility), keyEquivalent: "")
        item.target = self; menu.addItem(item)
        return menu
    }
    @objc private func openAccessibility() { onOpenAccessibility?() }
    @objc private func toggleSpeechMode() { onToggleSpeech?() }
    @objc private func toggleAutoMode() { onToggleAuto?() }
    private func updateModeAppearance() {
        pillContent.accent = autoMode ? TalkDesign.automatic : phase == .recording ? TalkDesign.active : TalkDesign.accent
        identity.color = autoMode ? TalkDesign.automatic : TalkDesign.accent
        wave.color = autoMode ? TalkDesign.automatic : TalkDesign.active
        pillContent.toolTip = autoMode
            ? "Auto · Focus a message field · Enter to submit · Esc to cancel · Hyper+V to turn off · \(DictationShortcut.help)"
            : "\(DictationShortcut.help) · Hyper+V: Auto mode · Click to start / stop · Drag to move · Right-click for modes"
    }
    private func updateAccessibilityLabel() {
        let action = waitingForAudio ? "Opening microphone" : phase == .recording ? "Stop dictation" : phase == .processing ? "Transcribing" : "Start dictation"
        pillContent.setAccessibilityLabel(autoMode ? "Auto mode · \(action)" : action)
    }
    private func layout(animated: Bool) {
        let duration = phase == .recording ? 0.16 : 0.3
        let showConnecting = !connecting.isHidden
        let size = notice.map { NSSize(width: max($0.frame.width, showConnecting ? 200 : 104), height: $0.frame.height + 36) }
            ?? (showConnecting ? NSSize(width: 200, height: 36) : expanded ? TalkDesign.recordingSize : TalkDesign.idleSize)
        resizePanel(for: size)
        let width = size.width, height = size.height
        let center = root.bounds.midX
        let frame = NSRect(x: center - width / 2, y: 12, width: width, height: height)
        glow.frame = CGRect(x: center - 30, y: 10, width: 60, height: 40)
        halo.frame = CGRect(x: center - 32, y: 8, width: 64, height: 44)
        halo.path = CGPath(roundedRect: CGRect(x: 4, y: 4, width: 56, height: 36), cornerWidth: 18, cornerHeight: 18, transform: nil)
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
        NSAnimationContext.runAnimationGroup { context in
            context.duration = animated && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? duration : 0
            context.timingFunction = CAMediaTimingFunction(controlPoints: 0.2, 0, 0, 1)
            pill.animator().frame = frame
            let contentOrigin = (width - (showConnecting ? 200 : TalkDesign.recordingSize.width)) / 2
            identity.animator().frame = NSRect(x: expanded ? contentOrigin + 9 : (width - 22) / 2, y: 7, width: 22, height: 22)
            wave.animator().frame = NSRect(x: contentOrigin + 39, y: 6, width: 52, height: 24)
            connecting.frame = NSRect(x: contentOrigin + 102, y: 10, width: 90, height: 17)
        }
        pillContent.frame = NSRect(origin: .zero, size: frame.size)
        if let notice { notice.frame.origin = NSPoint(x: (width - notice.frame.width) / 2, y: 36) }
        noticeRecordButton.frame = NSRect(x: (width - 36) / 2, y: 0, width: 36, height: 36)
        noticeRecordButton.setAccessibilityLabel(phase == .recording ? "Stop dictation" : "Start dictation")
    }
    private func resizePanel(for size: NSSize) {
        let windowSize = NSSize(width: max(200, size.width + 96), height: size.height + 48)
        guard panel.frame.size != windowSize else { return }
        let delta = (windowSize.width - root.bounds.width) / 2
        var frame = NSRect(x: panel.frame.midX - windowSize.width / 2, y: panel.frame.minY, width: windowSize.width, height: windowSize.height)
        if let screen = panel.screen {
            let visible = screen.visibleFrame
            frame.origin.x = min(max(frame.minX, visible.minX), visible.maxX - frame.width)
            frame.origin.y = min(max(frame.minY, visible.minY), visible.maxY - frame.height)
        }
        panel.setFrame(frame, display: false)
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
        guard phase == .recording, !waitingForAudio, enabled else { return }
        let ticker = Timer(timeInterval: 0.08, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.wave.heights.removeFirst()
            self.wave.heights.append(Waveform.height(self.level()))
        }
        ticker.tolerance = 0.01
        RunLoop.main.add(ticker, forMode: .common); timer = ticker
    }
    @objc private func recordFromNotice() { onToggle?() }
    private func clearNotice() {
        noticeDismiss?.cancel(); noticeDismiss = nil
        notice?.removeFromSuperview(); notice = nil
        modeNotice = false
        noticeRecordButton.isHidden = true
        pillContent.setAccessibilityRole(.button)
    }
    private func dismissNotice(animated: Bool = true) {
        guard notice != nil else { return }
        clearNotice()
        updateAccessibilityLabel()
        layout(animated: animated)
    }
    func error(_ message: String, accessibilityAction: Bool = false) {
        presentNotice(message, accessibilityAction: accessibilityAction, isModeNotice: false)
    }
    func showModeNotice() {
        let message = autoMode ? "Auto mode on" : "Auto mode off"
        presentNotice(message, accessibilityAction: false, isModeNotice: true)
    }
    private func presentNotice(_ message: String, accessibilityAction: Bool, isModeNotice: Bool) {
        guard enabled else { return }
        clearNotice()
        modeNotice = isModeNotice
        let action: (() -> Void)? = accessibilityAction ? { [weak self] in self?.onOpenAccessibility?() } : nil
        let content = OverlayNotice(message: message, openAccessibility: action, dismiss: { [weak self] in self?.dismissNotice() }, compact: isModeNotice)
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
        if !accessibilityAction {
            let dismiss = DispatchWorkItem { [weak self] in self?.dismissNotice() }
            noticeDismiss = dismiss
            DispatchQueue.main.asyncAfter(deadline: .now() + (isModeNotice ? 3 : 6), execute: dismiss)
        }
    }
}
