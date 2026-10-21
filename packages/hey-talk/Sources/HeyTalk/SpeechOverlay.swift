import AppKit

private final class SpeechPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}
private final class SpeechButton: NSButton {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .pointingHand) }
}
private final class SpeechFace: GlassFace {
    var reading = false { didSet { emphasized = reading || hovered } }
    private var hovered = false
    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.activeAlways, .mouseEnteredAndExited, .inVisibleRect], owner: self))
        super.updateTrackingAreas()
    }
    override func mouseEntered(with event: NSEvent) { hovered = true; emphasized = true }
    override func mouseExited(with event: NSEvent) { hovered = false; emphasized = reading }
}
private final class SpeechLabel: NSTextField {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}
private final class SpeechIcon: NSImageView {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

final class SpeechOverlay: NSObject {
    private let panel: SpeechPanel
    private let face = SpeechFace()
    private let play = SpeechButton(), stop = SpeechButton(), speed = SpeechButton(), disclosure = SpeechButton()
    private let label = SpeechLabel(labelWithString: "Listen")
    private let idleIcon = SpeechIcon()
    private let progress = NSProgressIndicator()
    private let scroll = NSScrollView()
    private let textView = NSTextView()
    private let glass: NSView
    private var target: SpeechTarget?
    private var snapshot: NSAttributedString?
    private var snapshotText: String?
    private var highlighted: NSRange?
    private var highlightedWord = false
    private var drawerOpen = true
    private var lastState: SpeechState = .idle
    private var lastRate: Float = 1
    private var lastFraction: Double = 0
    private var lastError: String?
    private var pinnedOrigin: CGPoint?
    var onPlay: (() -> Void)?
    var onStop: (() -> Void)?
    var onSpeed: (() -> Void)?
    var containsPointer: Bool { panel.isVisible && panel.frame.insetBy(dx: -8, dy: -8).contains(NSEvent.mouseLocation) }
    var visible: Bool { panel.isVisible }
    var frame: CGRect { panel.frame }
    func retainsHover(around target: SpeechTarget, point: CGPoint) -> Bool {
        guard panel.isVisible else { return false }
        let top = NSScreen.screens.first?.frame.maxY ?? 0
        let control = CGRect(x: panel.frame.minX, y: top - panel.frame.maxY, width: panel.frame.width, height: panel.frame.height)
        let paragraph = target.viewport.map { target.frame.intersection($0) } ?? target.frame
        return Self.hoverRegion(paragraph: paragraph, control: control).contains(point)
    }

    override init() {
        panel = SpeechPanel(contentRect: NSRect(x: 0, y: 0, width: 96, height: 36), styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        if #available(macOS 26, *) {
            let view = NSGlassEffectView(); view.style = .clear; view.cornerRadius = 18
            view.contentView = face; glass = view
        } else {
            let view = NSVisualEffectView(); view.material = .popover; view.blendingMode = .behindWindow; view.state = .active
            view.wantsLayer = true; view.layer?.cornerRadius = 18; view.layer?.masksToBounds = true
            face.autoresizingMask = [.width, .height]; view.addSubview(face); glass = view
        }
        super.init()
        panel.title = "Hey Talk — Read Aloud"
        panel.level = .floating; panel.isOpaque = false; panel.backgroundColor = .clear
        panel.hasShadow = true; panel.hidesOnDeactivate = false; panel.isReleasedWhenClosed = false
        panel.isMovableByWindowBackground = true
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        panel.contentView = glass
        for button in [play, stop, speed, disclosure] {
            button.isBordered = false; button.bezelStyle = .inline
            button.contentTintColor = TalkDesign.accent
            button.target = self; face.addSubview(button)
        }
        play.action = #selector(playClicked); stop.action = #selector(stopClicked); speed.action = #selector(speedClicked)
        disclosure.action = #selector(disclosureClicked)
        stop.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Stop reading")
        stop.setAccessibilityLabel("Stop reading"); stop.toolTip = "Stop · Esc"
        speed.font = .systemFont(ofSize: 11, weight: .semibold)
        speed.setAccessibilityLabel("Reading speed"); speed.toolTip = "Change reading speed"
        label.font = .systemFont(ofSize: 12, weight: .medium); label.textColor = .labelColor
        label.lineBreakMode = .byTruncatingTail; label.maximumNumberOfLines = 1
        face.addSubview(label)
        idleIcon.image = NSImage(systemSymbolName: "play.fill", accessibilityDescription: nil)
        idleIcon.contentTintColor = TalkDesign.accent
        idleIcon.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 11, weight: .semibold)
        face.addSubview(idleIcon)
        progress.style = .bar; progress.isIndeterminate = false; progress.minValue = 0; progress.maxValue = 1
        progress.controlSize = .mini; face.addSubview(progress)
        textView.isEditable = false; textView.isSelectable = false; textView.drawsBackground = false
        textView.isRichText = true; textView.textContainerInset = NSSize(width: 0, height: 4)
        textView.isVerticallyResizable = true; textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width]; textView.textContainer?.widthTracksTextView = true
        textView.setAccessibilityLabel("Passage being read")
        scroll.documentView = textView; scroll.drawsBackground = false
        scroll.hasVerticalScroller = true; scroll.autohidesScrollers = true
        face.addSubview(scroll)
    }
    func show(target: SpeechTarget, state: SpeechState, rate: Float, fraction: Double, error: String?) {
        let active = [.loading, .playing, .paused].contains(state)
        let expanded = active || state == .failed
        if active && (!lastState.isActive || snapshotText != target.text) {
            textView.subviews.filter { $0 is SpeechWordLens }.forEach { $0.removeFromSuperview() }
            snapshot = SpeechFormatting.captured(target); snapshotText = target.text
            textView.textStorage?.setAttributedString(snapshot!)
            highlighted = nil; drawerOpen = true; pinnedOrigin = nil
        }
        self.target = target; lastState = state; lastRate = rate; lastFraction = fraction; lastError = error
        let top = NSScreen.screens.first?.frame.maxY ?? 0
        let rect = CGRect(x: target.frame.minX, y: top - target.frame.maxY, width: target.frame.width, height: target.frame.height)
        let point = CGPoint(x: target.point.x, y: top - target.point.y)
        let screen = NSScreen.screens.first(where: { $0.frame.contains(point) }) ?? NSScreen.main
        guard let screen else { return }
        let drawer = expanded && drawerOpen && snapshot != nil
        let width: CGFloat = drawer ? min(max(360, target.frame.width + 32), min(680, screen.visibleFrame.width - 32)) : expanded ? 264 : 96
        let textHeight = snapshot?.boundingRect(with: CGSize(width: width - 32, height: .greatestFiniteMagnitude), options: [.usesLineFragmentOrigin, .usesFontLeading]).height ?? target.frame.height
        let height: CGFloat = drawer ? min(320, max(140, textHeight + 78)) : expanded ? 42 : 36
        var placement = Self.placement(anchor: rect, size: CGSize(width: width, height: height), visible: screen.visibleFrame)
        if drawer {
            let origin = pinnedOrigin ?? placement.origin
            placement.origin = origin
            pinnedOrigin = origin
        }
        // Respect dragging during playback; only size changes reposition the drawer.
        if active, panel.isVisible, panel.frame.size == placement.size { placement.origin = panel.frame.origin }
        panel.setFrame(placement, display: true)
        face.frame = CGRect(origin: .zero, size: placement.size)
        if #available(macOS 26, *), let glass = glass as? NSGlassEffectView { glass.style = drawer ? .regular : .clear }
        let rowY = height - 36
        play.frame = expanded ? CGRect(x: 5, y: rowY + 4, width: 30, height: 28) : CGRect(origin: .zero, size: placement.size)
        play.imagePosition = .imageOnly
        play.title = ""
        play.font = .systemFont(ofSize: 12, weight: .medium)
        play.image = expanded ? NSImage(systemSymbolName: state == .playing ? "pause.fill" : "play.fill", accessibilityDescription: nil) : nil
        play.setAccessibilityLabel(state == .playing ? "Pause reading" : state == .paused ? "Resume reading" : "Read paragraph")
        play.toolTip = "Read aloud · Hyper+S\n" + String(target.text.prefix(160))
        idleIcon.isHidden = expanded
        label.textColor = expanded ? .labelColor : TalkDesign.accent
        label.stringValue = !expanded ? "Listen" : state == .loading ? "Preparing…" : state == .paused ? "Paused" : state == .failed ? "Try again" : "Reading"
        if expanded {
            label.frame = CGRect(x: 40, y: rowY + 11, width: width - 160, height: 17)
        } else {
            let textSize = label.fittingSize
            let leading = floor((width - textSize.width - 18) / 2)
            idleIcon.frame = CGRect(x: leading, y: 12, width: 12, height: 12)
            label.frame = CGRect(x: leading + 18, y: floor((height - textSize.height) / 2), width: textSize.width, height: textSize.height)
        }
        label.toolTip = error
        disclosure.frame = CGRect(x: width - 104, y: rowY + 4, width: 30, height: 28)
        disclosure.image = NSImage(systemSymbolName: drawer ? "chevron.down" : "text.alignleft", accessibilityDescription: nil)
        disclosure.setAccessibilityLabel(drawer ? "Collapse passage" : "Show passage"); disclosure.isHidden = !expanded
        speed.frame = CGRect(x: width - 76, y: rowY + 4, width: 40, height: 28)
        speed.title = String(format: "%g×", rate); speed.isHidden = !active
        stop.frame = CGRect(x: width - 33, y: rowY + 4, width: 26, height: 28); stop.isHidden = !expanded
        progress.frame = CGRect(x: 41, y: rowY + 5, width: width - 160, height: 3)
        progress.doubleValue = fraction; progress.isHidden = !active
        scroll.isHidden = !drawer
        if drawer {
            scroll.frame = CGRect(x: 16, y: 14, width: width - 32, height: height - 58)
            textView.frame.size.width = scroll.contentSize.width
            textView.textContainer?.containerSize = NSSize(width: scroll.contentSize.width, height: .greatestFiniteMagnitude)
            if let container = textView.textContainer, let layout = textView.layoutManager {
                layout.ensureLayout(for: container)
                textView.frame.size.height = max(scroll.contentSize.height, layout.usedRect(for: container).height + 8)
            }
        }
        face.reading = active
        if !panel.isVisible { panel.orderFrontRegardless() }
    }
    func highlight(_ next: (range: NSRange, word: Bool)?) {
        guard drawerOpen, let snapshot, let storage = textView.textStorage,
              highlighted != next?.range || highlightedWord != (next?.word ?? false) else { return }
        textView.subviews.filter { $0 is SpeechWordLens }.forEach { $0.removeFromSuperview() }
        if let old = highlighted, NSMaxRange(old) <= snapshot.length {
            storage.replaceCharacters(in: old, with: snapshot.attributedSubstring(from: old))
        }
        highlighted = next?.range; highlightedWord = next?.word ?? false
        if let next, NSMaxRange(next.range) <= storage.length {
            textView.scrollRangeToVisible(next.range)
            guard let layout = textView.layoutManager, let container = textView.textContainer else { return }
            let glyphs = layout.glyphRange(forCharacterRange: next.range, actualCharacterRange: nil)
            let origin = textView.textContainerOrigin
            layout.enumerateLineFragments(forGlyphRange: glyphs) { _, _, _, line, _ in
                let part = NSIntersectionRange(line, glyphs)
                guard part.length > 0 else { return }
                let characters = layout.characterRange(forGlyphRange: part, actualGlyphRange: nil)
                let content = snapshot.attributedSubstring(from: characters)
                var rect = layout.boundingRect(forGlyphRange: part, in: container)
                rect.origin.x += origin.x; rect.origin.y += origin.y
                let lens = SpeechWordLens(text: content, word: next.word, frame: rect.insetBy(dx: -3, dy: -1))
                self.textView.addSubview(lens)
            }
            storage.addAttribute(.foregroundColor, value: NSColor.clear, range: next.range)
        }
    }
    static func hoverRegion(paragraph: CGRect, control: CGRect) -> CGRect {
        paragraph.union(control).insetBy(dx: -12, dy: -8)
    }
    static func placement(anchor: CGRect, size: CGSize, visible: CGRect) -> CGRect {
        // A paragraph owns one fixed button. Pointer motion must never chase it.
        let right = anchor.maxX + 8, left = anchor.minX - size.width - 8
        let preferred = right + size.width <= visible.maxX - 8 ? right : left >= visible.minX + 8 ? left : anchor.maxX - size.width
        let x = min(max(preferred, visible.minX + 8), visible.maxX - size.width - 8)
        let y = min(max(anchor.maxY - size.height, visible.minY + 8), visible.maxY - size.height - 8)
        return CGRect(origin: CGPoint(x: x, y: y), size: size)
    }
    func hide() { panel.orderOut(nil); target = nil; pinnedOrigin = nil }
    @objc private func playClicked() { onPlay?() }
    @objc private func stopClicked() { onStop?() }
    @objc private func speedClicked() { onSpeed?() }
    @objc private func disclosureClicked() {
        guard let target else { return }
        drawerOpen.toggle(); show(target: target, state: lastState, rate: lastRate, fraction: lastFraction, error: lastError)
    }
}
private extension SpeechState {
    var isActive: Bool { [.loading, .playing, .paused].contains(self) }
}
