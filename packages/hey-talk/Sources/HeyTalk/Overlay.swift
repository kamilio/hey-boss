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
        NSColor.labelColor.setFill()
        for (index, height) in heights.enumerated() {
            NSBezierPath(roundedRect: NSRect(x: CGFloat(index * 5), y: (bounds.height - height) / 2, width: 2, height: height), xRadius: 1, yRadius: 1).fill()
        }
    }
}

private final class IdentityView: NSView {
    override func draw(_ dirtyRect: NSRect) {
        NSColor.labelColor.setStroke()
        let scale = bounds.width / 24
        let ring = NSBezierPath(ovalIn: NSRect(x: 2 * scale, y: 2 * scale, width: 20 * scale, height: 20 * scale))
        ring.lineWidth = 2 * scale; ring.stroke()
        for (x, height) in [(8.75, 4.0), (12.0, 8.0), (15.25, 4.0)] {
            let bar = NSBezierPath()
            bar.move(to: NSPoint(x: x * scale, y: (12 - height / 2) * scale))
            bar.line(to: NSPoint(x: x * scale, y: (12 + height / 2) * scale))
            bar.lineWidth = 2 * scale; bar.lineCapStyle = .round; bar.stroke()
        }
    }
}

private final class ClickSurface: NSView {
    var onClick: (() -> Void)?
    var onHover: ((Bool) -> Void)?
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
    private let halo = CAShapeLayer()
    private var timer: Timer?
    private var phase: Phase = .idle
    private var hovering = false
    private var generation: UInt64 = 0
    private var expanded = false
    private var enabled = true
    private var toast: NSPanel?
    var onToggle: (() -> Void)?
    var onCancel: (() -> Void)?
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
        glow.type = .conic
        glow.startPoint = CGPoint(x: 0.5, y: 0.5); glow.endPoint = CGPoint(x: 0.5, y: 0)
        glow.colors = [NSColor.systemBlue.withAlphaComponent(0.25).cgColor, NSColor.systemIndigo.withAlphaComponent(0.55).cgColor, NSColor.systemBlue.cgColor, NSColor.systemCyan.cgColor, NSColor.white.cgColor]
        glow.locations = [0, 0.45, 0.75, 0.94, 1]
        let mask = CAShapeLayer()
        mask.path = CGPath(ellipseIn: CGRect(x: 1.25, y: 1.25, width: 41.5, height: 41.5), transform: nil)
        mask.fillColor = nil; mask.strokeColor = NSColor.white.cgColor; mask.lineWidth = 2.5
        glow.mask = mask
        halo.fillColor = NSColor.systemBlue.withAlphaComponent(0.3).cgColor
        halo.shadowColor = NSColor.systemBlue.cgColor; halo.shadowOpacity = 0.7; halo.shadowRadius = 7
        halo.shadowOffset = .zero
        glow.isHidden = true; halo.isHidden = true
        root.layer?.addSublayer(halo)
        root.layer?.addSublayer(glow)
        if #available(macOS 26, *) {
            let glass = NSGlassEffectView(); glass.style = .regular; glass.cornerRadius = 20
            glass.contentView = pillContent; pill = glass
            let cancel = NSGlassEffectView(); cancel.style = .regular; cancel.cornerRadius = 14
            cancel.contentView = cancelButton; cancelGlass = cancel
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
            pill = glass(pillContent, radius: 20); cancelGlass = glass(cancelButton, radius: 14)
            root.addSubview(pill); root.addSubview(cancelGlass)
        }
        pill.wantsLayer = true
        pillContent.addSubview(identity); pillContent.addSubview(wave)
        pillContent.setAccessibilityElement(true); pillContent.setAccessibilityRole(.button)
        pillContent.setAccessibilityLabel("Start dictation")
        pillContent.toolTip = "Press \(DictationShortcut.hint) or click to start / stop · Drag to move"
        pillContent.onClick = { [weak self] in self?.onToggle?() }
        pillContent.onHover = { [weak self] hover in self?.hovering = hover; self?.showCancel() }
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
        if visible { panel.orderFrontRegardless() } else { panel.orderOut(nil) }
        updateTimer()
    }
    func update(_ next: Phase) {
        generation &+= 1; let ticket = generation
        phase = next; expanded = false
        wave.heights = Array(repeating: 4, count: Waveform.count)
        wave.alphaValue = 0
        identity.layer?.removeAllAnimations()
        glow.removeAllAnimations(); halo.removeAllAnimations()
        glow.isHidden = next != .processing; halo.isHidden = next != .processing
        pillContent.setAccessibilityLabel(next == .recording ? "Stop dictation" : next == .processing ? "Transcribing" : "Start dictation")
        layout(animated: true); showCancel(); updateTimer()
        if next == .recording {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.26) { [weak self] in
                guard let self, self.generation == ticket else { return }
                self.expanded = true; self.layout(animated: true)
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { [weak self] in
                    guard let self, self.generation == ticket else { return }
                    NSAnimationContext.runAnimationGroup { context in context.duration = 0.2; self.wave.animator().alphaValue = 1 }
                }
            }
        } else if next == .processing {
            if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
                let spin = CABasicAnimation(keyPath: "transform.rotation.z")
                spin.fromValue = 0; spin.toValue = 2 * Double.pi; spin.duration = 1.6; spin.repeatCount = .infinity
                glow.add(spin, forKey: "orbit")
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
        let width: CGFloat = expanded ? 98 : 40, height: CGFloat = expanded ? 36 : 40
        let frame = NSRect(x: 100 - width / 2, y: 12, width: width, height: height)
        glow.frame = CGRect(x: 78, y: 10, width: 44, height: 44)
        halo.frame = CGRect(x: 76, y: 8, width: 48, height: 48)
        halo.path = CGPath(ellipseIn: CGRect(x: 4, y: 4, width: 40, height: 40), transform: nil)
        NSAnimationContext.runAnimationGroup { context in
            context.duration = animated && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0.3 : 0
            context.timingFunction = CAMediaTimingFunction(controlPoints: 0.2, 0, 0, 1)
            pill.animator().frame = frame
            identity.animator().frame = NSRect(x: expanded ? 6 : 9, y: (height - 22) / 2, width: 22, height: 22)
            wave.animator().frame = NSRect(x: 34, y: (height - 24) / 2, width: 52, height: 24)
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
    func error(_ message: String) {
        guard enabled else { return }
        toast?.orderOut(nil)
        let alert = RecordingPanel(contentRect: NSRect(x: panel.frame.midX - 170, y: panel.frame.maxY + 8, width: 340, height: 88), styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        alert.level = .statusBar; alert.isOpaque = false; alert.backgroundColor = .clear; alert.hasShadow = true
        alert.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        let surface = NSVisualEffectView(frame: NSRect(x: 0, y: 0, width: 340, height: 88))
        surface.material = .popover; surface.state = .active; surface.wantsLayer = true
        surface.layer?.cornerRadius = 16; surface.layer?.masksToBounds = true
        let label = NSTextField(wrappingLabelWithString: message)
        label.frame = NSRect(x: 16, y: 12, width: 308, height: 64)
        label.font = .systemFont(ofSize: 13); surface.addSubview(label)
        alert.contentView = surface; alert.orderFrontRegardless(); toast = alert
        DispatchQueue.main.asyncAfter(deadline: .now() + 8) { [weak alert] in alert?.orderOut(nil) }
    }
}
