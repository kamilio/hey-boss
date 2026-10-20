import AppKit
import Carbon
import TalkCore

final class Hotkey {
    private var handler: EventHandlerRef?
    private var shortcuts: [EventHotKeyRef] = []
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
            else {
                let action = pressed ? owner.state.keyDown(code: Int(id.id), hyper: true, repeated: false) : owner.state.keyUp(code: Int(id.id))
                if action != .none { owner.onAction?(action) }
            }
            return noErr
        }, 2, &types, Unmanaged.passUnretained(self).toOpaque(), &handler)
        let codes = DictationShortcut.allCases.map(\.keyCode) + [9] // Hyper+V toggles Auto.
        for code in codes {
            var ref: EventHotKeyRef?
            let id = EventHotKeyID(signature: 0x48544C4B, id: UInt32(code))
            if RegisterEventHotKey(UInt32(code), UInt32(controlKey | cmdKey | optionKey | shiftKey), id, GetApplicationEventTarget(), 0, &ref) == noErr, let ref {
                shortcuts.append(ref)
            }
        }
        registered = shortcuts.count == codes.count
    }
    func captureEscape(_ capture: Bool) {
        if capture, escape == nil {
            escapeRegistered = RegisterEventHotKey(53, 0, EventHotKeyID(signature: 0x48544C4B, id: 2), GetApplicationEventTarget(), 0, &escape) == noErr
        } else if !capture, let escape { UnregisterEventHotKey(escape); self.escape = nil; escapeRegistered = false }
    }
    func reset() { captureEscape(false) }
    deinit {
        for shortcut in shortcuts { UnregisterEventHotKey(shortcut) }
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
    func message(targetName: String? = nil, pasted: Bool = false) -> String {
        let name = targetName?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let app = name.isEmpty ? "the original app" : name
        if pasted { return "Pasted in \(app). Press Enter there." }
        switch self {
        case .accessibility: return "Text copied. Allow automatic paste."
        case .targetClosed: return "Reopen \(app). Press ⌘V."
        case .focusChanged: return "Return to \(app). Press ⌘V."
        case .ownWindow: return "Choose an app, then press ⌘V."
        case .eventUnavailable: return "Couldn’t paste. Press ⌘V."
        case .clipboardUnavailable: return "Clipboard unavailable. Open History."
        }
    }
}

enum Paste {
    static let eventTag: Int64 = 0x48544C4B
    static func deliver(_ text: String, target: NSRunningApplication?, pressEnter: Bool,
                        targetStillFocused: @escaping () -> Bool = { true },
                        isCurrent: @escaping () -> Bool, completion: @escaping (String, PasteBlockReason?) -> Void) {
        let board = NSPasteboard.general
        board.clearContents()
        guard board.setString(text, forType: .string) else { completion("history", .clipboardUnavailable); return }
        // Never redirect a dictation into an app the user switched to while decoding.
        send(pressEnter: pressEnter, check: { PasteBlockReason.check(trusted: AXIsProcessTrusted(),
            targetAlive: target.map { !$0.isTerminated } ?? false,
            targetIsSelf: target?.processIdentifier == getpid(),
            targetIsFrontmost: target != nil && NSWorkspace.shared.frontmostApplication?.processIdentifier == target?.processIdentifier)
                ?? (targetStillFocused() ? nil : .focusChanged)
        }, isCurrent: isCurrent, post: post, afterPaste: { action in
            // Let the receiving app consume the paste before sending Return.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.1, execute: action)
        }, completion: completion)
    }

    static func send(pressEnter: Bool, check: @escaping () -> PasteBlockReason?, isCurrent: @escaping () -> Bool,
                     post: @escaping (CGKeyCode, CGEventFlags) -> Bool, afterPaste: (@escaping () -> Void) -> Void,
                     completion: @escaping (String, PasteBlockReason?) -> Void) {
        if let reason = check() { completion("clipboard", reason); return }
        guard post(9, .maskCommand) else { completion("clipboard", .eventUnavailable); return }
        guard pressEnter else { completion("pasted", nil); return }
        afterPaste {
            guard isCurrent() else { completion("pasted", nil); return }
            if let reason = check() { completion("pasted", reason); return }
            guard post(36, []) else { completion("pasted", .eventUnavailable); return }
            completion("submitted", nil)
        }
    }

    private static func post(_ key: CGKeyCode, flags: CGEventFlags) -> Bool {
        guard let source = CGEventSource(stateID: .combinedSessionState),
              let down = CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: false) else { return false }
        down.flags = flags; up.flags = flags
        down.setIntegerValueField(.eventSourceUserData, value: eventTag)
        up.setIntegerValueField(.eventSourceUserData, value: eventTag)
        down.post(tap: .cghidEventTap); up.post(tap: .cghidEventTap)
        return true
    }
}
