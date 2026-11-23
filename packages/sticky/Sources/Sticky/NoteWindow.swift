import AppKit

// Like the dictation pill: always visible when shown, never the keyboard target.
private final class StickyPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}
private final class NoteHeader: NSView {
    var moved: (() -> Void)?
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override var mouseDownCanMoveWindow: Bool { true }
    override func mouseDown(with event: NSEvent) { window?.performDrag(with: event); moved?() }
}
private final class ResizeHandle: NSView {
    var resized: (() -> Void)?
    private var originalFrame: NSRect?
    private var origin = NSPoint.zero
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .frameResize(position: .bottomRight, directions: .all)) }
    override func mouseDown(with event: NSEvent) { originalFrame = window?.frame; origin = window?.convertPoint(toScreen: event.locationInWindow) ?? .zero }
    override func mouseDragged(with event: NSEvent) {
        guard let window, let frame = originalFrame else { return }
        let point = window.convertPoint(toScreen: event.locationInWindow)
        let width = max(window.minSize.width, frame.width + point.x - origin.x)
        let height = max(window.minSize.height, frame.height - point.y + origin.y)
        window.setFrame(NSRect(x: frame.minX, y: frame.maxY - height, width: width, height: height), display: true)
    }
    override func mouseUp(with event: NSEvent) { originalFrame = nil; resized?() }
}

final class NoteWindow: NSObject, NSWindowDelegate {
    let window: NSPanel
    let renderer = NoteRenderer()
    private let heading = NSTextField(labelWithString: "")
    private var note: Note
    private let saveNote: (Note) throws -> Void
    private var editor: NoteEditor?
    private let placement: NotePlacement
    var dirty: Bool { editor?.dirty ?? false }
    var id: String { note.id }
    init(note: Note, position: Int, save: @escaping (Note) throws -> Void) throws {
        self.note = note; self.saveNote = save
        placement = NotePlacement(id: note.id, position: position)
        let panel = StickyPanel(contentRect: NSRect(x: 0, y: 0, width: 400, height: 420), styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        window = panel
        super.init()
        panel.isFloatingPanel = true; panel.becomesKeyOnlyIfNeeded = true
        panel.hidesOnDeactivate = false
        panel.level = .statusBar
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        panel.backgroundColor = .clear; panel.isOpaque = false
        // A borderless resizable NSWindow adds a square frame/shadow outside glass.
        panel.hasShadow = false
        panel.isReleasedWhenClosed = false
        panel.minSize = NSSize(width: 300, height: 240); panel.delegate = self
        let content = NSView(frame: NSRect(origin: .zero, size: panel.frame.size))
        content.autoresizingMask = [.width, .height]
        let glass = NSGlassEffectView(frame: content.bounds)
        glass.style = .regular; glass.cornerRadius = 22
        glass.contentView = content; panel.contentView = glass
        let header = NoteHeader(frame: NSRect(x: 0, y: 358, width: 400, height: 62))
        header.moved = { [weak self] in self?.saveFrame() }
        header.autoresizingMask = [.width, .minYMargin]; content.addSubview(header)
        heading.frame = NSRect(x: 26, y: 17, width: 310, height: 27)
        heading.font = .systemFont(ofSize: 19, weight: .semibold); heading.textColor = .labelColor
        heading.lineBreakMode = .byTruncatingTail; heading.autoresizingMask = [.width]
        heading.isSelectable = false; header.addSubview(heading)
        let close = NSButton(frame: NSRect(x: 345, y: 18, width: 30, height: 28))
        close.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Hide note")
        close.imageScaling = .scaleProportionallyDown; close.isBordered = false
        close.contentTintColor = .secondaryLabelColor; close.refusesFirstResponder = true
        close.toolTip = "Hide note · Hyper+N to show again"; close.setAccessibilityLabel("Hide note")
        close.target = self; close.action = #selector(hideNote); close.autoresizingMask = [.minXMargin]
        header.addSubview(close)
        renderer.web.frame = NSRect(x: 0, y: 10, width: 400, height: 348)
        renderer.web.autoresizingMask = [.width, .height]; content.addSubview(renderer.web)
        let resize = ResizeHandle(frame: NSRect(x: 374, y: 4, width: 20, height: 20))
        resize.resized = { [weak self] in self?.saveFrame() }
        resize.autoresizingMask = [.minXMargin, .maxYMargin]; content.addSubview(resize)
        renderer.ready = { [weak self] result in
            if case .failure(let error) = result { self?.heading.toolTip = error.localizedDescription }
        }
        try render(note)
        restoreFrame()
        NotificationCenter.default.addObserver(self, selector: #selector(screensChanged), name: NSApplication.didChangeScreenParametersNotification, object: nil)
    }
    private func render(_ value: Note) throws {
        note = value; window.title = value.title; heading.stringValue = value.title
        heading.toolTip = value.title; window.setAccessibilityLabel(value.title)
        try renderer.load(value.markdown)
    }
    func update(_ value: Note) throws {
        guard !dirty else { throw StickyError("This note has unsaved edits. Save or discard them before replacing it.") }
        try render(value); editor?.update(value)
    }
    func edit() {
        if editor == nil {
            editor = NoteEditor(note: note, save: { [weak self] value in
                guard let self else { throw StickyError("The note is no longer available.") }
                try self.saveNote(value); try self.render(value)
            }, close: { [weak self] in self?.editor = nil })
        }
        editor?.show()
    }
    func show() { restoreFrame(); window.orderFrontRegardless() }
    @objc func hideNote() { window.orderOut(nil) }
    func windowShouldClose(_ sender: NSWindow) -> Bool { hideNote(); return false }
    @objc private func screensChanged() {
        // Let AppKit finish moving windows after a display change before restoring ours.
        DispatchQueue.main.async { [weak self] in self?.restoreFrame() }
    }
    private func restoreFrame() {
        if let frame = placement.frame(on: NoteScreen.current) { window.setFrame(frame, display: true) }
    }
    private func saveFrame() {
        // Only user drags/resizes save positions; AppKit's disconnect fallback must not overwrite them.
        placement.save(window.frame, on: NoteScreen.current)
    }
    func invalidate() { NotificationCenter.default.removeObserver(self); renderer.invalidate(); window.orderOut(nil) }
}
