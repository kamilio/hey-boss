import AppKit

/// Native equivalents of --accent and --active in src/issues/web/components.css.
enum TalkDesign {
    static let accent = adaptive(light: 0x4655b5, dark: 0xa1afff)
    static let onAccent = adaptive(light: 0xffffff, dark: 0x18204c)
    static let active = adaptive(light: 0x168179, dark: 0x80cfca)
    static let automatic = adaptive(light: 0x8250ad, dark: 0xd4b0ff)
    static let idleSize = NSSize(width: 56, height: 36)
    static let recordingSize = NSSize(width: 104, height: 36)
    static var speechOffImage: NSImage? {
        NSImage(systemSymbolName: "speaker.slash.fill", accessibilityDescription: "Read aloud off")?
            .withSymbolConfiguration(.init(paletteColors: [.systemRed]))
    }
    private static func adaptive(light: UInt32, dark: UInt32) -> NSColor {
        NSColor(name: nil) { appearance in
            let value = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light
            return NSColor(srgbRed: CGFloat((value >> 16) & 255) / 255,
                           green: CGFloat((value >> 8) & 255) / 255,
                           blue: CGFloat(value & 255) / 255, alpha: 1)
        }
    }
}

/// Optical edge over native glass, with no opaque fill or idle animation.
class GlassFace: NSView {
    var accent: NSColor = TalkDesign.accent { didSet { needsDisplay = true } }
    var emphasized = false { didSet { needsDisplay = true } }
    override var isOpaque: Bool { false }
    override func draw(_ dirtyRect: NSRect) {
        let rect = bounds.insetBy(dx: 0.75, dy: 0.75)
        let radius = min(18, rect.height / 2)
        let outer = NSBezierPath(roundedRect: rect, xRadius: radius, yRadius: radius)
        NSGraphicsContext.saveGraphicsState()
        outer.addClip()
        NSGradient(starting: accent.withAlphaComponent(emphasized ? 0.18 : 0.10),
                   ending: accent.withAlphaComponent(0.015))?.draw(in: rect, angle: 90)
        NSGraphicsContext.restoreGraphicsState()
        let inner = NSBezierPath(roundedRect: rect.insetBy(dx: 0.8, dy: 0.8), xRadius: radius - 0.8, yRadius: radius - 0.8)
        outer.append(inner); outer.windingRule = .evenOdd
        NSGraphicsContext.saveGraphicsState()
        outer.addClip()
        NSGradient(colorsAndLocations: (.white.withAlphaComponent(0.82), 0),
                   (accent.withAlphaComponent(0.32), 0.42),
                   (.white.withAlphaComponent(0.15), 0.62),
                   (.white.withAlphaComponent(0.92), 1))?.draw(in: rect, angle: 90)
        NSGraphicsContext.restoreGraphicsState()
    }
    override func setFrameSize(_ newSize: NSSize) { super.setFrameSize(newSize); needsDisplay = true }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
}
