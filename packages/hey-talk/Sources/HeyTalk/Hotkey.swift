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
            if id.id == 2, pressed { owner.onAction?(.cancel) }
            else if id.id == 1 {
                let action = pressed ? owner.state.keyDown(code: DictationShortcut.keyCode, hyper: true, repeated: false) : owner.state.keyUp(code: DictationShortcut.keyCode)
                if action != .none { owner.onAction?(action) }
            }
            return noErr
        }, 2, &types, Unmanaged.passUnretained(self).toOpaque(), &handler)
        let id = EventHotKeyID(signature: 0x48544C4B, id: 1)
        registered = RegisterEventHotKey(UInt32(DictationShortcut.keyCode), UInt32(controlKey | cmdKey | optionKey | shiftKey), id, GetApplicationEventTarget(), 0, &shortcut) == noErr
    }
    func captureEscape(_ capture: Bool) {
        if capture, escape == nil {
            escapeRegistered = RegisterEventHotKey(53, 0, EventHotKeyID(signature: 0x48544C4B, id: 2), GetApplicationEventTarget(), 0, &escape) == noErr
        } else if !capture, let escape { UnregisterEventHotKey(escape); self.escape = nil; escapeRegistered = false }
    }
    func reset() { captureEscape(false) }
    deinit {
        if let shortcut { UnregisterEventHotKey(shortcut) }
        if let escape { UnregisterEventHotKey(escape) }
        if let handler { RemoveEventHandler(handler) }
    }
}

enum PasteBlockReason: String {
    case accessibility, targetClosed = "target_closed", focusChanged = "focus_changed"
    case ownWindow = "own_window", eventUnavailable = "event_unavailable", clipboardUnavailable = "clipboard_unavailable"

    static func check(trusted: Bool, targetAlive: Bool, targetIsSelf: Bool, targetIsFrontmost: Bool) -> Self? {
        if !trusted { return .accessibility }
        if !targetAlive { return .targetClosed }
        if targetIsSelf { return .ownWindow }
        if !targetIsFrontmost { return .focusChanged }
        return nil
    }
    var message: String {
        switch self {
        case .accessibility: return "macOS hasn’t granted this build Accessibility access. Open Automatic Paste from the Hey Talk menu. Text saved and copied; ⌘V to paste."
        case .targetClosed: return "The original app closed. Text saved and copied; ⌘V to paste."
        case .focusChanged: return "You switched apps, so automatic paste was skipped. Text saved and copied; ⌘V to paste."
        case .ownWindow: return "Start dictating from the app you want to paste into. Text saved and copied; ⌘V to paste."
        case .eventUnavailable: return "macOS couldn’t send the paste shortcut. Text saved and copied; ⌘V to paste."
        case .clipboardUnavailable: return "The clipboard is unavailable. Your text is saved in Dictation History."
        }
    }
}

enum Paste {
    static func deliver(_ text: String, target: NSRunningApplication?, completion: @escaping (String, PasteBlockReason?) -> Void) {
        let board = NSPasteboard.general
        board.clearContents()
        guard board.setString(text, forType: .string) else { completion("history", .clipboardUnavailable); return }
        // Never redirect a dictation into an app the user switched to while decoding.
        if let reason = PasteBlockReason.check(trusted: AXIsProcessTrusted(),
            targetAlive: target.map { !$0.isTerminated } ?? false,
            targetIsSelf: target?.processIdentifier == getpid(),
            targetIsFrontmost: target != nil && NSWorkspace.shared.frontmostApplication?.processIdentifier == target?.processIdentifier) {
            completion("clipboard", reason); return
        }
        guard let source = CGEventSource(stateID: .combinedSessionState),
              let down = CGEvent(keyboardEventSource: source, virtualKey: 9, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: 9, keyDown: false) else { completion("clipboard", .eventUnavailable); return }
        down.flags = .maskCommand; up.flags = .maskCommand
        down.post(tap: .cghidEventTap); up.post(tap: .cghidEventTap)
        completion("pasted", nil)
    }
}
