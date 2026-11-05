import AppKit
import Carbon
import TalkCore

final class Hotkey {
    private var handler: EventHandlerRef?
    private var shortcuts: [EventHotKeyRef] = []
    private var escape: EventHotKeyRef?
    private var speech: EventHotKeyRef?
    private var speechMode: EventHotKeyRef?
    private var state = HotkeyState()
    var onAction: ((HotkeyAction) -> Void)?
    private(set) var registered = false
    private(set) var escapeRegistered = false
    private(set) var eventsReceived = 0
    var speechRegistered: Bool { speech != nil }
    var speechModeRegistered: Bool { speechMode != nil }
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
        let codes = DictationShortcut.allCases.map(\.keyCode) + [9, 46] // Hyper+V: Auto; Hyper+M: chat with context.
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
    func enableSpeech(_ enabled: Bool) {
        if enabled, speech == nil {
            var ref: EventHotKeyRef?
            if RegisterEventHotKey(1, UInt32(controlKey | cmdKey | optionKey | shiftKey), EventHotKeyID(signature: 0x48544C4B, id: 1), GetApplicationEventTarget(), 0, &ref) == noErr { speech = ref }
        } else if !enabled, let speech { UnregisterEventHotKey(speech); self.speech = nil; _ = state.keyUp(code: 1) }
    }
    func enableSpeechMode(_ allowed: Bool) {
        if allowed, speechMode == nil {
            var ref: EventHotKeyRef?
            if RegisterEventHotKey(17, UInt32(controlKey | cmdKey | optionKey | shiftKey), EventHotKeyID(signature: 0x48544C4B, id: 17), GetApplicationEventTarget(), 0, &ref) == noErr { speechMode = ref }
        } else if !allowed, let speechMode { UnregisterEventHotKey(speechMode); self.speechMode = nil; _ = state.keyUp(code: 17) }
    }
    func reset() { captureEscape(false) }
    deinit {
        for shortcut in shortcuts { UnregisterEventHotKey(shortcut) }
        if let escape { UnregisterEventHotKey(escape) }
        if let speech { UnregisterEventHotKey(speech) }
        if let speechMode { UnregisterEventHotKey(speechMode) }
        if let handler { RemoveEventHandler(handler) }
    }
}

enum PasteBlockReason: String {
    case accessibility, targetClosed = "target_closed", focusChanged = "focus_changed"
    case newThreadUnavailable = "new_thread_unavailable"
    case clearUnavailable = "clear_unavailable"
    case slackUnavailable = "slack_unavailable"
    case t3Unavailable = "t3_unavailable"
    case unidentifiedField = "unidentified_field"
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
        if pasted {
            return self == .focusChanged || self == .unidentifiedField
                ? "Pasted in \(app). Focus changed before submit." : "Pasted in \(app). Couldn’t send Enter."
        }
        switch self {
        case .clearUnavailable: return "Couldn’t clear the original field."
        case .slackUnavailable: return "Couldn’t navigate in Slack."
        case .t3Unavailable: return "Couldn’t finish the T3 action. Check the selected chat."
        case .newThreadUnavailable: return "Couldn’t open a new thread. Check the project in T3."
        case .accessibility: return "Text copied. Allow automatic paste."
        case .targetClosed: return "Reopen \(app). Press ⌘V."
        case .focusChanged: return "Return to \(app). Press ⌘V."
        case .unidentifiedField: return "Text copied. Choose the field and press ⌘V."
        case .ownWindow: return "Choose an app, then press ⌘V."
        case .eventUnavailable: return "Couldn’t paste. Press ⌘V."
        case .clipboardUnavailable: return "Clipboard unavailable. Open History."
        }
    }
}

enum Paste {
    static let eventTag: Int64 = 0x48544C4B
    static func copyAndStopWaiting(_ text: String, retry: Timer, to board: NSPasteboard) -> Bool {
        board.clearContents()
        guard board.setString(text, forType: .string) else { return false }
        retry.invalidate()
        return true
    }
    static func deliver(_ action: CorrectionAction, target: NSRunningApplication?, originalField: FocusedInput?, shouldPressEnter: @escaping () -> Bool, returnToField: FocusedInput?,
                        targetStillFocused: @escaping () -> Bool, onWait: ((Timer) -> Void)?,
                        isCurrent: @escaping () -> Bool, onNewField: ((FocusedInput) -> Void)? = nil, listedThreads: [T3ThreadRow]? = nil,
                        completion: @escaping (String, PasteBlockReason?) -> Void) {
        func check() -> PasteBlockReason? {
            PasteBlockReason.check(trusted: AXIsProcessTrusted(), targetAlive: target.map { !$0.isTerminated } ?? false,
                targetIsSelf: target?.processIdentifier == getpid(),
                targetIsFrontmost: target != nil && NSWorkspace.shared.frontmostApplication?.processIdentifier == target?.processIdentifier)
                ?? (targetStillFocused() ? nil : onWait == nil ? .unidentifiedField : .focusChanged)
        }
        if action == .clearInput {
            Task { @MainActor in
                guard let field = originalField, let target else { completion("history", .clearUnavailable); return }
                func focused() -> AXUIElement? {
                    guard isCurrent(), target.isActive, let input = FocusedInput.read(app: target, forAuto: false), input == field else { return nil }
                    return input.element
                }
                guard let editor = focused(), let value = SpeechAX.value(editor, "AXValue") as? String else { completion("history", .clearUnavailable); return }
                if value.isEmpty { completion("cleared", nil); return }
                guard post(0, flags: .maskCommand, to: target.processIdentifier) else { completion("history", .clearUnavailable); return }
                var deleted = false
                for _ in 0..<20 {
                    try? await Task.sleep(nanoseconds: 25_000_000)
                    guard let editor = focused(), let contents = SpeechAX.value(editor, "AXValue") as? String else { break }
                    // Chromium can expose a trailing newline outside its editable text.
                    if deleted {
                        if contents.isEmpty || (contents == "\n" && field.composerLabel != nil) { completion("cleared", nil); return }
                    } else {
                        guard contents == value else { break }
                        guard let selected = SpeechAX.value(editor, "AXSelectedText") as? String,
                              selected == contents || (selected + "\n" == contents && field.composerLabel != nil) else { continue }
                        if selected.isEmpty { completion("cleared", nil); return }
                        guard post(51, flags: [], to: target.processIdentifier) else { break }
                        deleted = true
                    }
                }
                completion("history", .clearUnavailable)
            }
            return
        }
        func paste() {
            switch action {
            case .switchThread(let position):
                Task { @MainActor in
                    guard isCurrent(), check() == nil, let field = originalField,
                          let selected = await field.switchThread(position: position, listed: listedThreads, isCurrent: isCurrent) else {
                        completion("history", .t3Unavailable); return
                    }
                    onNewField?(selected)
                    completion("switched", nil)
                }
                return
            case .pinThread, .settleThread, .rename:
                Task { @MainActor in
                    guard isCurrent(), check() == nil, let field = originalField,
                          let id = field.t3ThreadID, let environment = field.t3EnvironmentID else {
                        completion("history", .t3Unavailable); return
                    }
                    do {
                        try await T3Server.shared.apply(action, thread: id, environment: environment, isCurrent: { isCurrent() && field.isFocused() })
                        if case .rename(let title) = action {
                            var renamed: FocusedInput?
                            for _ in 0..<40 {
                                guard isCurrent(), field.app.isActive else { break }
                                if let fresh = FocusedInput.read(app: field.app, forAuto: false), fresh.document == field.document,
                                   fresh.composerLabel == "Message " + title { renamed = fresh; break }
                                try? await Task.sleep(nanoseconds: 40_000_000)
                            }
                            guard let renamed else { completion("history", .t3Unavailable); return }
                            onNewField?(renamed)
                        }
                        completion(action == .settleThread ? "settled" : action == .pinThread(true) ? "pinned" : action == .pinThread(false) ? "unpinned" : "renamed", nil)
                    } catch { completion("history", .t3Unavailable) }
                }
                return
            case .listThreads: completion("history", .t3Unavailable); return
            default: break
            }
            if case .newThread(let project, let text) = action {
                Task { @MainActor in
                    guard isCurrent(), check() == nil, let field = originalField,
                          let draft = await field.openNewThread(project: project, isCurrent: isCurrent) else {
                        completion("history", .newThreadUnavailable); return
                    }
                    onNewField?(draft)
                    guard !text.isEmpty else { completion("new_thread", nil); return }
                    guard isCurrent(), draft.isFocused(),
                          SpeechAX.string(draft.element, "AXValue").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    else { completion("history", .newThreadUnavailable); return }
                    deliver(.paste(text), target: draft.app, originalField: draft, shouldPressEnter: shouldPressEnter,
                            returnToField: nil, targetStillFocused: draft.isFocused, onWait: nil, isCurrent: isCurrent, completion: completion)
                }
                return
            }
            let board = NSPasteboard.general
            board.clearContents()
            guard board.setString(action.text, forType: .string) else { completion("history", .clipboardUnavailable); return }
            func keypress(_ key: CGKeyCode, _ flags: CGEventFlags) -> Bool {
                guard let target else { return false }
                return post(key, flags: flags, to: target.processIdentifier)
            }
            if case .paste = action {
                send(shouldPressEnter: shouldPressEnter, check: check, isCurrent: isCurrent, post: keypress,
                     afterPaste: { DispatchQueue.main.asyncAfter(deadline: .now() + 0.1, execute: $0) }) { delivery, reason in
                    if delivery == "clipboard", reason == .focusChanged, let onWait { retry(onWait) }
                    else { completion(delivery, reason) }
                }
            }
        }
        func retry(_ onWait: (Timer) -> Void, restoring field: FocusedInput? = nil) {
            let deadline = ProcessInfo.processInfo.systemUptime + 2
            var expectedTab: AXUIElement?
            var focusedEditor = false
            let timer = Timer(timeInterval: 0.35, repeats: true) { timer in
                guard isCurrent() else { timer.invalidate(); return }
                if check() == .focusChanged {
                    if let field, !focusedEditor, ProcessInfo.processInfo.systemUptime < deadline {
                        if let expectedTab, SpeechAX.value(expectedTab, kAXSelectedAttribute) as? Bool != true { return }
                        if let next = field.stepTowardCmuxTab() { expectedTab = next }
                        else { focusedEditor = field.focusReturnedField() }
                    }
                    return
                }
                timer.invalidate()
                paste()
            }
            timer.tolerance = 0.05
            RunLoop.main.add(timer, forMode: .common)
            onWait(timer)
        }
        if let onWait, check() == .focusChanged {
            retry(onWait, restoring: returnToField)
            returnToField?.restoreFocus()
        } else { paste() }
    }

    static func send(shouldPressEnter: @escaping () -> Bool, check: @escaping () -> PasteBlockReason?, isCurrent: @escaping () -> Bool,
                     post: @escaping (CGKeyCode, CGEventFlags) -> Bool, afterPaste: (@escaping () -> Void) -> Void,
                     completion: @escaping (String, PasteBlockReason?) -> Void) {
        if let reason = check() { completion("clipboard", reason); return }
        guard post(9, .maskCommand) else { completion("clipboard", .eventUnavailable); return }
        guard shouldPressEnter() else { completion("pasted", nil); return }
        afterPaste {
            guard isCurrent() else { completion("pasted", nil); return }
            if let reason = check() { completion("pasted", reason); return }
            guard shouldPressEnter() else { completion("pasted", nil); return }
            guard post(36, []) else { completion("pasted", .eventUnavailable); return }
            completion("submitted", nil)
        }
    }

    static func post(_ key: CGKeyCode, flags: CGEventFlags, to pid: pid_t) -> Bool {
        guard let source = CGEventSource(stateID: .combinedSessionState),
              let down = CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: false) else { return false }
        down.flags = flags; up.flags = flags
        down.setIntegerValueField(.eventSourceUserData, value: eventTag)
        up.setIntegerValueField(.eventSourceUserData, value: eventTag)
        down.postToPid(pid); up.postToPid(pid)
        return true
    }
}
