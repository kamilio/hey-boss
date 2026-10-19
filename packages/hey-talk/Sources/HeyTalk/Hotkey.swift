import AppKit
import Carbon
import TalkCore

final class Hotkey {
    private var handler: EventHandlerRef?
    private var shortcut: EventHotKeyRef?
    private var escape: EventHotKeyRef?
    private var state = HotkeyState()
    var onAction: ((HotkeyAction) -> Void)?
    private(set) var registered = false
    private(set) var escapeRegistered = false
    private(set) var eventsReceived = 0
    init() {
        var types = [EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed)), EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyReleased))]
        InstallEventHandler(GetApplicationEventTarget(), { _, event, user in
            guard let event, let user else { return OSStatus(eventNotHandledErr) }
            let owner = Unmanaged<Hotkey>.fromOpaque(user).takeUnretainedValue()
            owner.eventsReceived += 1
            var id = EventHotKeyID()
            GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), nil, MemoryLayout<EventHotKeyID>.size, nil, &id)
            let pressed = GetEventKind(event) == UInt32(kEventHotKeyPressed)
            if id.id == 2, pressed { owner.state.reset(); owner.onAction?(.cancel) }
            else if id.id == 1 {
                let action = pressed ? owner.state.keyDown(code: 14, hyper: true, repeated: false) : owner.state.keyUp(code: 14)
                if action != .none { owner.onAction?(action) }
            }
            return noErr
        }, 2, &types, Unmanaged.passUnretained(self).toOpaque(), &handler)
        let id = EventHotKeyID(signature: 0x48544C4B, id: 1)
        registered = RegisterEventHotKey(14, UInt32(controlKey | cmdKey | optionKey | shiftKey), id, GetApplicationEventTarget(), 0, &shortcut) == noErr
    }
    func captureEscape(_ capture: Bool) {
        if capture, escape == nil {
            escapeRegistered = RegisterEventHotKey(53, 0, EventHotKeyID(signature: 0x48544C4B, id: 2), GetApplicationEventTarget(), 0, &escape) == noErr
        } else if !capture, let escape { UnregisterEventHotKey(escape); self.escape = nil; escapeRegistered = false }
    }
    func reset() { state.reset(); captureEscape(false) }
    deinit {
        if let shortcut { UnregisterEventHotKey(shortcut) }
        if let escape { UnregisterEventHotKey(escape) }
        if let handler { RemoveEventHandler(handler) }
    }
}

enum Paste {
    static func deliver(_ text: String, target: NSRunningApplication?, completion: @escaping (String) -> Void) {
        let board = NSPasteboard.general
        board.clearContents(); board.setString(text, forType: .string)
        guard AXIsProcessTrusted() else { completion("clipboard"); return }
        guard let target, !target.isTerminated else { completion("clipboard"); return }
        // The overlay never activates. If the user moved to another app during
        // decoding, leave text on the clipboard instead of pasting elsewhere.
        guard NSWorkspace.shared.frontmostApplication?.processIdentifier == target.processIdentifier else { completion("clipboard"); return }
        guard let source = CGEventSource(stateID: .combinedSessionState),
              let down = CGEvent(keyboardEventSource: source, virtualKey: 9, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: 9, keyDown: false) else { completion("clipboard"); return }
        down.flags = .maskCommand; up.flags = .maskCommand
        down.post(tap: .cghidEventTap); up.post(tap: .cghidEventTap)
        completion("pasted")
    }
}
