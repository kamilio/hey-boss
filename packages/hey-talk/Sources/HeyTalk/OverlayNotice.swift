import AppKit

/// Buttons stay in AppKit's action chain even though the recording panel never
/// becomes key. Opening Settings requires an explicit click; errors never focus it.
final class OverlayNotice: NSView {
    private let openAccessibility: (() -> Void)?
    private let dismiss: () -> Void

    init(message: String, openAccessibility: (() -> Void)?, dismiss: @escaping () -> Void) {
        self.openAccessibility = openAccessibility
        self.dismiss = dismiss
        super.init(frame: NSRect(x: 0, y: 0, width: 360, height: openAccessibility == nil ? 108 : 140))
        let label = NSTextField(wrappingLabelWithString: message)
        label.font = .systemFont(ofSize: 13)
        label.drawsBackground = false
        label.frame = NSRect(x: 18, y: openAccessibility == nil ? 16 : 52, width: 324, height: 72)
        addSubview(label)
        if openAccessibility != nil {
            let settings = NSButton(title: "Open Accessibility Settings", target: self, action: #selector(openSettings))
            settings.bezelStyle = .rounded; settings.controlSize = .small
            settings.font = .systemFont(ofSize: 12, weight: .medium)
            settings.frame = NSRect(x: 16, y: 14, width: 230, height: 26)
            addSubview(settings)
            let close = NSButton(title: "Dismiss", target: self, action: #selector(closeNotice))
            close.bezelStyle = .rounded; close.controlSize = .small
            close.font = .systemFont(ofSize: 12)
            close.frame = NSRect(x: 264, y: 14, width: 80, height: 26)
            addSubview(close)
        }
    }
    required init?(coder: NSCoder) { nil }
    @objc private func openSettings() { dismiss(); openAccessibility?() }
    @objc private func closeNotice() { dismiss() }
}
