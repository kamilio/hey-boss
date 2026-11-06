import AppKit

final class PasteWaitView: NSView {
    private let cancel: () -> Void
    init(instruction: String, cancel: @escaping () -> Void) {
        self.cancel = cancel
        super.init(frame: NSRect(x: 0, y: 0, width: 286, height: 54))
        let title = NSTextField(labelWithString: "Waiting to paste")
        title.font = .systemFont(ofSize: 13, weight: .semibold)
        title.frame = NSRect(x: 43, y: 28, width: 202, height: 17)
        let detail = NSTextField(labelWithString: instruction)
        detail.font = .systemFont(ofSize: 12); detail.textColor = .secondaryLabelColor
        detail.lineBreakMode = .byTruncatingTail
        detail.frame = NSRect(x: 43, y: 10, width: 202, height: 16)
        toolTip = detail.stringValue
        let spinner = NSProgressIndicator(frame: NSRect(x: 16, y: 19, width: 16, height: 16))
        spinner.style = .spinning; spinner.controlSize = .small
        if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion { spinner.startAnimation(nil) }
        let close = NSButton(image: NSImage(systemSymbolName: "xmark.circle.fill", accessibilityDescription: "Copy and close")!, target: self, action: #selector(cancelPaste))
        close.isBordered = false; close.contentTintColor = .secondaryLabelColor
        close.toolTip = "Copy and close (Esc)"; close.setAccessibilityLabel("Copy and close")
        close.frame = NSRect(x: 249, y: 13, width: 27, height: 28)
        for view in [spinner, title, detail, close] { addSubview(view) }
        setAccessibilityRole(.group)
        setAccessibilityLabel(title.stringValue + ". " + detail.stringValue + ". Escape to copy and close.")
    }
    required init?(coder: NSCoder) { nil }
    @objc private func cancelPaste() { cancel() }
}

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
    private let performAction: (() -> Void)?
    private let dismiss: () -> Void

    init(message: String, action: (title: String, perform: () -> Void)? = nil, retry: (() -> Void)? = nil, dismiss: @escaping () -> Void, compact: Bool = false) {
        performAction = retry ?? action?.perform
        self.dismiss = dismiss
        let label = NSTextField(wrappingLabelWithString: message)
        label.font = .systemFont(ofSize: compact ? 12 : 13, weight: compact ? .medium : .regular)
        label.maximumNumberOfLines = compact ? 1 : 4
        label.sizeToFit()
        let width: CGFloat = compact ? ceil(label.frame.width) + 38 : 312
        let textWidth = compact ? label.frame.width : width - (retry == nil ? 58 : 84)
        label.preferredMaxLayoutWidth = textWidth
        let textHeight = ceil(label.intrinsicContentSize.height)
        let actionsHeight: CGFloat = action == nil ? 0 : 40
        let height: CGFloat = compact ? 36 : textHeight + 28 + actionsHeight
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: height))
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel(compact ? message : "Dictation notice")
        label.frame = NSRect(x: compact ? 2 : 14, y: 14 + actionsHeight, width: textWidth, height: textHeight)
        if compact {
            label.frame.origin.y = (18 + label.firstBaselineOffsetFromTop - label.frame.height - label.font!.capHeight / 2).rounded()
        }
        addSubview(label)
        if !compact {
            let separator = NSBox()
            separator.boxType = .separator
            separator.frame = NSRect(x: 16, y: 0, width: width - 32, height: 1)
            separator.alphaValue = 0.3
            addSubview(separator)
        }
        let capCenter = label.frame.maxY - label.firstBaselineOffsetFromTop + label.font!.capHeight / 2
        func addIcon(_ symbol: String, _ title: String, x: CGFloat, selector: Selector) {
            let button = NSButton(image: NSImage(systemSymbolName: symbol, accessibilityDescription: title)!, target: self, action: selector)
            button.isBordered = false
            button.contentTintColor = .secondaryLabelColor
            button.setAccessibilityLabel(title)
            button.toolTip = title
            button.symbolConfiguration = .init(pointSize: 10, weight: .medium)
            button.frame = NSRect(x: x, y: (capCenter - 12).rounded(), width: 24, height: 24)
            addSubview(button)
        }
        addIcon("xmark", "Dismiss notice", x: width - 34, selector: #selector(closeNotice))
        if retry != nil { addIcon("arrow.clockwise", "Retry", x: width - 60, selector: #selector(activateAction)) }
        if let action {
            let button = NoticeActionButton(title: action.title, target: self, action: #selector(activateAction))
            button.isBordered = false
            button.font = .systemFont(ofSize: 13, weight: .semibold)
            button.frame = NSRect(x: 16, y: 12, width: width - 32, height: 32)
            addSubview(button)
        }
    }
    required init?(coder: NSCoder) { nil }
    override func mouseDown(with event: NSEvent) {}
    override func mouseUp(with event: NSEvent) {}
    @objc private func activateAction() { dismiss(); performAction?() }
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
