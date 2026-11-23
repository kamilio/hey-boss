import AppKit

private final class EditorWindow: NSWindow {
    var save: (() -> Void)?
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.modifierFlags.intersection([.command, .shift, .option, .control]) == .command {
            if event.charactersIgnoringModifiers == "s" { save?(); return true }
            if event.charactersIgnoringModifiers == "w" { performClose(nil); return true }
        }
        return super.performKeyEquivalent(with: event)
    }
}

final class NoteEditor: NSObject, NSWindowDelegate {
    let window: NSWindow
    private let text = NSTextView()
    private var note: Note
    private let saveNote: (Note) throws -> Void
    private let onClose: () -> Void
    private var closing = false
    var dirty: Bool { text.string != note.markdown }
    init(note: Note, save: @escaping (Note) throws -> Void, close: @escaping () -> Void) {
        self.note = note; saveNote = save; onClose = close
        let panel = EditorWindow(contentRect: NSRect(x: 0, y: 0, width: 760, height: 620), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window = panel
        super.init()
        panel.title = note.title + " — Markdown"
        panel.titlebarAppearsTransparent = true; panel.titlebarSeparatorStyle = .none
        panel.isReleasedWhenClosed = false; panel.minSize = NSSize(width: 480, height: 360)
        panel.delegate = self; panel.save = { [weak self] in self?.save() }
        let content = NSView(frame: NSRect(origin: .zero, size: panel.frame.size))
        panel.contentView = content
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 56, width: 760, height: 564))
        scroll.autoresizingMask = [.width, .height]; scroll.hasVerticalScroller = true
        scroll.borderType = .noBorder
        text.frame = scroll.contentView.bounds
        text.minSize = NSSize(width: 0, height: 0)
        text.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        text.isVerticallyResizable = true; text.isHorizontallyResizable = false
        text.autoresizingMask = [.width]; text.textContainer?.widthTracksTextView = true
        text.textContainerInset = NSSize(width: 28, height: 24)
        text.font = .monospacedSystemFont(ofSize: 15, weight: .regular)
        text.isRichText = false; text.allowsUndo = true
        text.isAutomaticQuoteSubstitutionEnabled = false; text.isAutomaticDashSubstitutionEnabled = false
        text.isAutomaticTextReplacementEnabled = false; text.isAutomaticSpellingCorrectionEnabled = false
        text.backgroundColor = .textBackgroundColor; text.textColor = .textColor; text.insertionPointColor = .textColor
        text.setAccessibilityLabel("Note Markdown")
        text.string = note.markdown
        scroll.documentView = text; content.addSubview(scroll)
        let hint = NSTextField(labelWithString: "Markdown · first heading becomes the note title")
        hint.font = .systemFont(ofSize: 12); hint.textColor = .secondaryLabelColor
        hint.frame = NSRect(x: 24, y: 20, width: 450, height: 17); hint.autoresizingMask = [.width]
        content.addSubview(hint)
        let button = NSButton(title: "Save", target: self, action: #selector(NoteEditor.save))
        button.bezelStyle = .rounded; button.frame = NSRect(x: 650, y: 12, width: 88, height: 32)
        button.autoresizingMask = [.minXMargin]; content.addSubview(button)
        panel.center()
    }
    func show() {
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil); window.makeFirstResponder(text)
    }
    func update(_ value: Note) {
        note = value; text.string = value.markdown
        window.title = value.title + " — Markdown"
    }
    @objc private func save() {
        do {
            let value = Note(id: note.id, markdown: text.string)
            try saveNote(value)
            note = value; window.title = value.title + " — Markdown"
        } catch {
            let alert = NSAlert(error: error); alert.beginSheetModal(for: window)
        }
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard dirty && !closing else { return true }
        let alert = NSAlert(); alert.messageText = "Save changes to \(note.title)?"
        alert.addButton(withTitle: "Save"); alert.addButton(withTitle: "Discard"); alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { [weak self] response in
            guard let self else { return }
            if response == .alertFirstButtonReturn {
                self.save(); if self.dirty { return }
            } else if response != .alertSecondButtonReturn { return }
            self.closing = true; self.window.performClose(nil)
        }
        return false
    }
    func windowWillClose(_ notification: Notification) { onClose() }
}
