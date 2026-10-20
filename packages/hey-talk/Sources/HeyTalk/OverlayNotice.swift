import AppKit

// Native button tint disappears in a non-key panel. Keep the primary action
// prominent without taking keyboard focus away from the user's app.
private final class NoticeActionButton: NSButton {
    override func draw(_ dirtyRect: NSRect) {
        let shape = NSBezierPath(roundedRect: bounds, xRadius: 10, yRadius: 10)
        TalkDesign.accent.setFill(); shape.fill()
        if isHighlighted { NSColor.black.withAlphaComponent(0.12).setFill(); shape.fill() }
        let text = NSAttributedString(string: title, attributes: [
            .font: font ?? NSFont.systemFont(ofSize: 13, weight: .semibold),
            .foregroundColor: TalkDesign.onAccent,
        ])
        let size = text.size()
        text.draw(at: NSPoint(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2))
    }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }
}

/// Buttons stay in AppKit's action chain even though the recording panel never
/// becomes key. Opening Settings requires an explicit click; errors never focus it.
final class OverlayNotice: NSView {
    private let openAccessibility: (() -> Void)?
    private let dismiss: () -> Void

    init(message: String, openAccessibility: (() -> Void)?, dismiss: @escaping () -> Void, compact: Bool = false) {
        self.openAccessibility = openAccessibility
        self.dismiss = dismiss
        super.init(frame: NSRect(x: 0, y: 0, width: compact ? 180 : 312, height: compact ? 40 : openAccessibility == nil ? 56 : 96))
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel("Dictation notice")
        let label = NSTextField(wrappingLabelWithString: message)
        label.font = .systemFont(ofSize: 12, weight: .medium)
        label.drawsBackground = false
        label.maximumNumberOfLines = 2
        label.frame = NSRect(x: 16, y: compact ? 10 : bounds.height - 45, width: bounds.width - 62, height: compact ? 20 : 34)
        addSubview(label)
        let separator = NSBox()
        separator.boxType = .separator
        separator.frame = NSRect(x: 16, y: 0, width: bounds.width - 32, height: 1)
        separator.alphaValue = 0.3
        addSubview(separator)
        let close = NSButton(image: NSImage(systemSymbolName: "xmark", accessibilityDescription: "Dismiss notice")!, target: self, action: #selector(closeNotice))
        close.isBordered = false
        close.contentTintColor = .secondaryLabelColor
        close.setAccessibilityLabel("Dismiss notice")
        close.toolTip = "Dismiss"
        close.frame = NSRect(x: bounds.width - 36, y: bounds.height - 33, width: 24, height: 24)
        addSubview(close)
        if openAccessibility != nil {
            let settings = NoticeActionButton(title: "Enable Accessibility", target: self, action: #selector(openSettings))
            settings.isBordered = false
            settings.font = .systemFont(ofSize: 13, weight: .semibold)
            settings.frame = NSRect(x: 16, y: 10, width: 280, height: 34)
            addSubview(settings)
        }
    }
    required init?(coder: NSCoder) { nil }
    override func mouseDown(with event: NSEvent) {}
    override func mouseUp(with event: NSEvent) {}
    @objc private func openSettings() { dismiss(); openAccessibility?() }
    @objc private func closeNotice() { dismiss() }

    static func concise(_ detail: String) -> String {
        let text = detail.lowercased()
        if text.contains("no speech") { return "No speech detected. Try again." }
        if text.contains("microphone access") { return "Allow microphone access in Settings." }
        if text.contains("no sound from the microphone") { return "No microphone sound. Check mute and Sound settings." }
        if text.contains("microphone") { return "Microphone unavailable. Try reconnecting." }
        if text.contains("five-minute") { return "Recording limit reached. Start again." }
        if text.contains("orukeet is missing") { return "Speech model missing. Reinstall Hey Talk." }
        return "Couldn’t transcribe. Try again."
    }
}
