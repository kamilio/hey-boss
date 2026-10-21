import AppKit

// This sits inside the frozen text view, so a tiny optical enlargement never
// reflows the paragraph or moves the words that follow it.
final class SpeechWordLens: NSView {
    let magnification: CGFloat
    private let ink: LensInk
    init(text: NSAttributedString, word: Bool, frame: CGRect) {
        magnification = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 1 : word ? 1.035 : 1
        ink = LensInk(text: text, scale: magnification)
        super.init(frame: frame)
        let surface: NSView
        if #available(macOS 26, *), !NSWorkspace.shared.accessibilityDisplayShouldReduceTransparency {
            let glass = NSGlassEffectView(); glass.style = .clear; glass.cornerRadius = 6
            glass.tintColor = TalkDesign.accent.withAlphaComponent(word ? 0.12 : 0.035)
            glass.contentView = ink; surface = glass
        } else {
            let background = NSView(); background.wantsLayer = true
            background.layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
            background.layer?.cornerRadius = 6
            background.addSubview(ink); surface = background
        }
        surface.frame = bounds; surface.autoresizingMask = [.width, .height]
        ink.frame = bounds; ink.autoresizingMask = [.width, .height]
        wantsLayer = true; layer?.cornerRadius = 6
        layer?.borderWidth = 0.5; layer?.borderColor = TalkDesign.accent.withAlphaComponent(word ? 0.2 : 0.07).cgColor
        addSubview(surface)
        setAccessibilityElement(false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}
private final class LensInk: NSView {
    let text: NSAttributedString, scale: CGFloat
    init(text: NSAttributedString, scale: CGFloat) { self.text = text; self.scale = scale; super.init(frame: .zero) }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        let size = text.size()
        NSGraphicsContext.saveGraphicsState()
        let transform = AffineTransform(translationByX: (bounds.width - size.width * scale) / 2,
                                        byY: (bounds.height - size.height * scale) / 2)
        (transform as NSAffineTransform).concat()
        let enlargement = NSAffineTransform(); enlargement.scale(by: scale); enlargement.concat()
        text.draw(at: .zero)
        NSGraphicsContext.restoreGraphicsState()
    }
}

final class SpeechSourceHighlight {
    private let panel: NSPanel
    init() {
        panel = NSPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.title = "Hey Talk — Reading passage"
        panel.isOpaque = false; panel.backgroundColor = .clear; panel.hasShadow = false
        panel.ignoresMouseEvents = true; panel.hidesOnDeactivate = false
        panel.level = NSWindow.Level(rawValue: NSWindow.Level.floating.rawValue - 1)
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        let surface: NSView
        if #available(macOS 26, *), !NSWorkspace.shared.accessibilityDisplayShouldReduceTransparency {
            let glass = NSGlassEffectView(); glass.style = .clear; glass.cornerRadius = 9
            glass.tintColor = TalkDesign.accent.withAlphaComponent(0.04)
            // Keep the source readable through the glass; stronger magnification
            // belongs in the frozen drawer, where glyph positions are exact.
            glass.alphaValue = 0.18; surface = glass
        } else { surface = NSView() }
        let face = NSView(); face.wantsLayer = true
        face.layer?.cornerRadius = 9; face.layer?.borderWidth = 0.75
        face.layer?.borderColor = TalkDesign.accent.withAlphaComponent(0.22).cgColor
        face.layer?.backgroundColor = TalkDesign.accent.withAlphaComponent(0.025).cgColor
        surface.autoresizingMask = [.width, .height]; face.addSubview(surface)
        panel.contentView = face
    }
    static func clipped(_ frame: CGRect, to viewport: CGRect) -> CGRect? {
        let clipped = frame.insetBy(dx: -4, dy: -4).intersection(viewport)
        return clipped.isNull || clipped.width < 8 || clipped.height < 8 ? nil : clipped
    }
    func show(frame: CGRect, viewport: CGRect, reading: Bool) {
        guard let rect = Self.clipped(frame, to: viewport) else { hide(); return }
        let top = NSScreen.screens.first?.frame.maxY ?? 0
        let native = CGRect(x: rect.minX, y: top - rect.maxY, width: rect.width, height: rect.height)
        guard NSScreen.screens.contains(where: { $0.frame.intersects(native) }) else { hide(); return }
        panel.setFrame(native, display: true)
        panel.contentView?.layer?.borderColor = TalkDesign.accent.withAlphaComponent(reading ? 0.22 : 0.13).cgColor
        panel.contentView?.layer?.backgroundColor = TalkDesign.accent.withAlphaComponent(reading ? 0.025 : 0.015).cgColor
        panel.contentView?.subviews.first?.frame = CGRect(origin: .zero, size: rect.size)
        if !panel.isVisible { panel.orderFrontRegardless() }
    }
    func hide() { panel.orderOut(nil) }
}
