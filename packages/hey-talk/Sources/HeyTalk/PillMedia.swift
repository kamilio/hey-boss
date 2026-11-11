import AppKit

/// The expanded part of the voice pill. Its bottom row remains the voice control.
final class PillMedia: NSView {
    private let title = NSTextField(labelWithString: "")
    private let kind = NSTextField(labelWithString: "")
    private let zoom = NSButton()
    let close = NSButton()
    private let scroll = NSScrollView()
    private final class Paper: NSView { override var isFlipped: Bool { true } }
    private final class Clip: NSClipView { override var isFlipped: Bool { true } }
    private let paper = Paper()
    private let picture = NSImageView()
    private var fit = false

    init(image: NSImage, title: String, kind: String) {
        super.init(frame: .zero); wantsLayer = true
        self.title.stringValue = title; self.title.toolTip = title
        self.title.font = .systemFont(ofSize: 16, weight: .semibold); self.title.lineBreakMode = .byTruncatingTail
        self.kind.stringValue = kind == "Image" ? "IMAGE" : kind.uppercased() + "  /  DIAGRAM"
        self.kind.font = .systemFont(ofSize: 10, weight: .bold); self.kind.textColor = .secondaryLabelColor
        zoom.controlSize = .small; zoom.bezelStyle = .accessoryBarAction; zoom.target = self; zoom.action = #selector(toggleFit)
        close.isBordered = false; close.image = NSImage(systemSymbolName: "xmark.circle.fill", accessibilityDescription: "Hide media")
        close.contentTintColor = .secondaryLabelColor; close.toolTip = "Hide media"
        scroll.contentView = Clip(); scroll.backgroundColor = .white; scroll.hasVerticalScroller = true; scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true; scroll.scrollerStyle = .overlay
        scroll.wantsLayer = true; scroll.layer?.cornerRadius = 10; scroll.layer?.masksToBounds = true
        scroll.documentView = paper; paper.addSubview(picture)
        picture.image = image; picture.setAccessibilityLabel(title); picture.imageScaling = .scaleAxesIndependently
        for view in [self.kind, self.title, zoom, close, scroll] { addSubview(view) }
        fit = kind == "Image"
    }
    required init?(coder: NSCoder) { fatalError() }
    override func mouseDown(with event: NSEvent) {}
    override func mouseUp(with event: NSEvent) {}
    func preferredSize(on screen: NSRect) -> NSSize {
        let image = picture.image!.size
        return NSSize(width: min(max(310, ceil(image.width) + 50), min(1080, screen.width - 100)),
                      height: min(max(172, ceil(image.height) + 90), screen.height * 0.62))
    }
    override func layout() {
        super.layout()
        let w = bounds.width, h = bounds.height
        close.frame = NSRect(x: w - 40, y: h - 39, width: 28, height: 26)
        scroll.frame = NSRect(x: 8, y: 1, width: max(0, w - 16), height: max(0, h - 55))
        let area = scroll.contentSize, image = picture.image!.size
        let fitScale = max(0, min(1, (area.width - 28) / image.width, (area.height - 28) / image.height))
        zoom.isHidden = fitScale >= 1; zoom.title = fit ? "Actual Size" : "Fit"; zoom.sizeToFit()
        zoom.setFrameOrigin(NSPoint(x: close.frame.minX - zoom.frame.width - 4, y: close.frame.midY - zoom.frame.height / 2))
        let width = max(0, (zoom.isHidden ? close : zoom).frame.minX - 24)
        kind.frame = NSRect(x: 16, y: h - 22, width: width, height: 14)
        title.frame = NSRect(x: 16, y: h - 44, width: width, height: 21)
        let scale = fit ? fitScale : 1, size = NSSize(width: image.width * scale, height: image.height * scale)
        paper.setFrameSize(NSSize(width: max(area.width, size.width + 28), height: max(area.height, size.height + 28)))
        picture.frame = NSRect(x: (paper.bounds.width - size.width)/2, y: (paper.bounds.height - size.height)/2, width: size.width, height: size.height)
    }
    @objc private func toggleFit() { fit.toggle(); needsLayout = true }
}
