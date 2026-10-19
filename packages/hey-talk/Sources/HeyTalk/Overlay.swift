import AppKit
import QuartzCore
import TalkCore

private final class RecordingPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

private final class WaveView: NSView {
    var heights = Array(repeating: 4.0, count: Waveform.count) { didSet { needsDisplay = true } }
    override func draw(_ dirtyRect: NSRect) {
        TalkDesign.active.setFill()
        for (index, height) in heights.enumerated() {
            NSBezierPath(roundedRect: NSRect(x: CGFloat(index * 5), y: (bounds.height - height) / 2, width: 2, height: height), xRadius: 1, yRadius: 1).fill()
        }
    }
}

private final class IdentityView: NSView {
    override func draw(_ dirtyRect: NSRect) {
        TalkDesign.accent.setStroke()
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
    var onOpenAccessibility: (() -> Void)?
    private var start: NSPoint?
    private var origin: NSPoint?
    private var dragged = false
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
        if dragged { window?.setFrameOrigin(NSPoint(x: origin.x + current.x - start.x, y: origin.y + current.y - start.y)) }
    }
    override func mouseUp(with event: NSEvent) { if !dragged { onClick?() }; start = nil }
    override func rightMouseDown(with event: NSEvent) {
        let menu = NSMenu()
        let item = NSMenuItem(title: "Automatic Paste…", action: #selector(openAccessibility), keyEquivalent: "")
        item.target = self; menu.addItem(item)
        NSMenu.popUpContextMenu(menu, with: event, for: self)
    }
    @objc private func openAccessibility() { onOpenAccessibility?() }
    override func accessibilityPerformPress() -> Bool { onClick?(); return true }
}

final class Overlay {
    private let panel: RecordingPanel
    private let root = NSView(frame: NSRect(x: 0, y: 0, width: 200, height: 84))
    private let pill: NSView
    private let pillContent = ClickSurface()
    private let identity = IdentityView()
    private let wave = WaveView()
    private let cancelGlass: NSView
    private let cancelButton = NSButton()
    private let glow = CAGradientLayer()
    private let depth = CAShapeLayer()
    private let halo = CAShapeLayer()
    private var timer: Timer?
    private var phase: Phase = .idle
    private var hovering = false
    private var expanded = false
    private var enabled = true
    private var toast: NSPanel?
    var onToggle: (() -> Void)?
    var onCancel: (() -> Void)?
    var onOpenAccessibility: (() -> Void)?
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
            let cancel = NSGlassEffectView(); cancel.style = .clear; cancel.cornerRadius = 14
            let face = GlassFace(); face.addSubview(cancelButton)
            cancel.contentView = face; cancelGlass = cancel
            let container = NSGlassEffectContainerView(frame: root.bounds)
            container.spacing = 8
            let content = NSView(frame: root.bounds)
            container.contentView = content; root.addSubview(container)
            content.addSubview(pill); content.addSubview(cancelGlass)
        } else {
            func glass(_ content: NSView, radius: CGFloat) -> NSView {
                let view = NSVisualEffectView(); view.material = .popover
                view.blendingMode = .behindWindow; view.state = .active
                view.wantsLayer = true; view.layer?.cornerRadius = radius; view.layer?.masksToBounds = true
                content.autoresizingMask = [.width, .height]; view.addSubview(content)
                return view
            }
            pill = glass(pillContent, radius: 18); cancelGlass = glass(cancelButton, radius: 14)
            root.addSubview(pill); root.addSubview(cancelGlass)
        }
        pill.wantsLayer = true
        // Keep the live waveform inside the capsule while it expands.
        pill.clipsToBounds = true
        wave.isHidden = true
        wave.identifier = NSUserInterfaceItemIdentifier("recording-waveform")
        pillContent.addSubview(identity); pillContent.addSubview(wave)
        pillContent.setAccessibilityElement(true); pillContent.setAccessibilityRole(.button)
        pillContent.setAccessibilityLabel("Start dictation")
        pillContent.toolTip = "\(DictationShortcut.help) · Click to start / stop · Drag to move"
        pillContent.onClick = { [weak self] in self?.onToggle?() }
        pillContent.onOpenAccessibility = { [weak self] in self?.onOpenAccessibility?() }
        pillContent.onHover = { [weak self] hover in self?.hovering = hover; self?.pillContent.emphasized = hover; self?.showCancel() }
        cancelButton.isBordered = false
        cancelButton.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Cancel dictation")
        cancelButton.imageScaling = .scaleNone
        cancelButton.target = self; cancelButton.action = #selector(cancel)
        cancelButton.toolTip = "Cancel · Esc"
        cancelGlass.alphaValue = 0; cancelGlass.isHidden = true
        layout(animated: false)
        reposition()
        NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in self?.reposition() }
    }
    func reposition() {
        let screen = NSScreen.screens.first(where: { NSMouseInRect(NSEvent.mouseLocation, $0.frame, false) }) ?? NSScreen.main
        guard let screen else { return }
        let visible = screen.visibleFrame
        panel.setFrameOrigin(NSPoint(x: visible.midX - 100, y: visible.minY + 12))
    }
    func show(_ visible: Bool) {
        enabled = visible
        if !visible { toast?.orderOut(nil) }
        if visible { panel.orderFrontRegardless() } else { panel.orderOut(nil) }
        updateTimer()
    }
    func update(_ next: Phase) {
        if next == .recording || next == .processing { toast?.orderOut(nil) }
        phase = next; expanded = next == .recording
        pillContent.accent = next == .recording ? TalkDesign.active : TalkDesign.accent
        wave.heights = Array(repeating: 4, count: Waveform.count)
        wave.isHidden = next != .recording
        wave.alphaValue = 1
        identity.layer?.removeAllAnimations()
        glow.removeAllAnimations(); halo.removeAllAnimations()
        glow.isHidden = next != .processing; halo.isHidden = next != .processing
        pillContent.setAccessibilityLabel(next == .recording ? "Stop dictation" : next == .processing ? "Transcribing" : "Start dictation")
        layout(animated: true); showCancel(); updateTimer()
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
        update(.processing)
        pillContent.setAccessibilityLabel("Opening microphone")
    }
    private func layout(animated: Bool) {
        let duration = phase == .recording ? 0.16 : 0.3
        let size = expanded ? TalkDesign.recordingSize : TalkDesign.idleSize
        let width = size.width, height = size.height
        let frame = NSRect(x: 100 - width / 2, y: 12, width: width, height: height)
        glow.frame = CGRect(x: 70, y: 10, width: 60, height: 40)
        halo.frame = CGRect(x: 68, y: 8, width: 64, height: 44)
        halo.path = CGPath(roundedRect: CGRect(x: 4, y: 4, width: 56, height: 36), cornerWidth: 18, cornerHeight: 18, transform: nil)
        let outline = CGPath(roundedRect: frame, cornerWidth: height / 2, cornerHeight: height / 2, transform: nil)
        if animated && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            for key in ["path", "shadowPath"] {
                let transition = CABasicAnimation(keyPath: key)
                transition.fromValue = depth.presentation()?.value(forKey: key) ?? depth.value(forKey: key)
                transition.toValue = outline; transition.duration = duration
                transition.timingFunction = CAMediaTimingFunction(controlPoints: 0.2, 0, 0, 1)
                depth.add(transition, forKey: key)
            }
        }
        CATransaction.begin(); CATransaction.setDisableActions(true)
        depth.path = outline; depth.shadowPath = outline
        CATransaction.commit()
        NSAnimationContext.runAnimationGroup { context in
            context.duration = animated && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? duration : 0
            context.timingFunction = CAMediaTimingFunction(controlPoints: 0.2, 0, 0, 1)
            pill.animator().frame = frame
            identity.animator().frame = NSRect(x: expanded ? 9 : (width - 22) / 2, y: (height - 22) / 2, width: 22, height: 22)
            wave.animator().frame = NSRect(x: 39, y: (height - 24) / 2, width: 52, height: 24)
            cancelGlass.animator().frame = NSRect(x: frame.maxX + 8, y: frame.midY - 14, width: 28, height: 28)
        }
        pillContent.frame = NSRect(origin: .zero, size: frame.size)
        cancelButton.frame = NSRect(x: 0, y: 0, width: 28, height: 28)
    }
    private func showCancel() {
        let visible = hovering && (phase == .recording || phase == .processing)
        if visible { cancelGlass.isHidden = false }
        // Keep the hit target alive while the pointer crosses the 8pt gap.
        if !visible {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.45) { [weak self] in
                guard let self, !self.hovering else { return }
                let local = self.panel.convertPoint(fromScreen: NSEvent.mouseLocation)
                if self.cancelGlass.frame.insetBy(dx: -8, dy: -8).contains(local) { return }
                self.cancelGlass.alphaValue = 0; self.cancelGlass.isHidden = true
            }
        } else { cancelGlass.alphaValue = 1 }
    }
    private func updateTimer() {
        timer?.invalidate(); timer = nil
        guard phase == .recording, enabled else { return }
        let ticker = Timer(timeInterval: 0.08, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.wave.heights.removeFirst()
            self.wave.heights.append(Waveform.height(self.level()))
        }
        ticker.tolerance = 0.01
        RunLoop.main.add(ticker, forMode: .common); timer = ticker
    }
    @objc private func cancel() { onCancel?() }
    func error(_ message: String, accessibilityAction: Bool = false) {
        guard enabled else { return }
        toast?.orderOut(nil)
        let alert = RecordingPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        alert.title = accessibilityAction ? "Hey Talk — Automatic Paste" : "Hey Talk — Notice"
        alert.level = .statusBar; alert.isOpaque = false; alert.backgroundColor = .clear; alert.hasShadow = true
        alert.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        alert.hidesOnDeactivate = false
        let action: (() -> Void)? = accessibilityAction ? { [weak self] in self?.onOpenAccessibility?() } : nil
        let content = OverlayNotice(message: message, openAccessibility: action, dismiss: { [weak alert] in alert?.orderOut(nil) })
        let surface = glassSurface(content: content, radius: 20)
        surface.frame = content.bounds
        alert.contentView = surface
        alert.setContentSize(content.bounds.size)
        alert.setFrameOrigin(NSPoint(x: panel.frame.midX - content.bounds.width / 2, y: panel.frame.maxY + 8))
        alert.orderFrontRegardless(); toast = alert
        // Keep the repair link available until clicked, dismissed, or recording resumes.
        if !accessibilityAction {
            DispatchQueue.main.asyncAfter(deadline: .now() + 8) { [weak alert] in alert?.orderOut(nil) }
        }
    }
}
