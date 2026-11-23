import AppKit
import Carbon
import os
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
        let codes = DictationShortcut.allCases.map(\.keyCode) + [8, 9, 46] // Hyper+C: attach selection; Hyper+V: insert last dictation; Hyper+M: catch up and talk.
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
    case ownWindow = "own_window", eventUnavailable = "event_unavailable"

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
        case .accessibility: return "Allow Accessibility, then press Hyper+V."
        case .targetClosed: return "Reopen \(app). Press Hyper+V."
        case .focusChanged: return "Return to \(app). Press Hyper+V."
        case .unidentifiedField: return "Choose a text field and press Hyper+V."
        case .ownWindow: return "Choose an app, then press Hyper+V."
        case .eventUnavailable: return "Couldn’t insert here. Choose another field or copy from History."
        }
    }
}

enum Paste {
    static let log = Logger(subsystem: "local.hey-talk", category: "paste")
    static let eventTag: Int64 = 0x48544C4B
    static func deliver(_ action: CorrectionAction, target: NSRunningApplication?, originalField: FocusedInput?, visibleThread: VisibleT3Thread? = nil, shouldPressEnter: @escaping () -> Bool, returnToField: FocusedInput?,
                        needsField: Bool, onWait: ((Timer) -> Void)?,
                        isCurrent: @escaping () -> Bool, onNewField: ((FocusedInput) -> Void)? = nil, listedThreads: [T3ThreadRow]? = nil,
                        completion: @escaping (String, PasteBlockReason?) -> Void) {
        let windowThread: VisibleT3Thread?
        switch action {
        case .pinThread, .settleThread, .rename: windowThread = visibleThread
        default: windowThread = nil
        }
        var previousState: String?

        func check() -> PasteBlockReason? {
            let front = FocusedInput.keyboardApp
            var reason = PasteBlockReason.check(trusted: AXIsProcessTrusted(), targetAlive: target.map { !$0.isTerminated } ?? false,
                targetIsSelf: target?.processIdentifier == getpid(), targetIsFrontmost: target != nil && front?.processIdentifier == target?.processIdentifier)
            var state = reason?.rawValue ?? "ready"
            if reason == nil {
                var mismatch: String?
                if let field = originalField {
                    mismatch = field.mismatch(FocusedInput.read(app: field.app, forAuto: false))
                    if let tab = field.cmuxTab, SpeechAX.value(tab, kAXSelectedAttribute) as? Bool != true { mismatch = "cmux_tab_changed" }
                } else if needsField { mismatch = "no_captured_field" }
                if let mismatch {
                    state = mismatch
                    reason = onWait == nil ? .unidentifiedField : .focusChanged
                }
            }
            if state != previousState {
                log.notice("Check: \(state, privacy: .public), target=\(target?.bundleIdentifier ?? "none", privacy: .public), front=\(front?.bundleIdentifier ?? "none", privacy: .public)")
                previousState = state
            }
            return reason
        }
        if action == .clearInput {
            Task { @MainActor in
                guard let field = originalField, let target else { completion("history", .clearUnavailable); return }
                func focused() -> AXUIElement? {
                    guard isCurrent(), FocusedInput.keyboardApp == target, let input = FocusedInput.read(app: target, forAuto: false), input == field else { return nil }
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
            case .switchThread, .nextThread:
                Task { @MainActor in
                    guard isCurrent(), check() == nil, let field = originalField else { completion("history", .t3Unavailable); return }
                    let selected: FocusedInput?
                    if case .switchThread(let position) = action {
                        selected = await field.switchThread(position: position, listed: listedThreads, isCurrent: isCurrent)
                    } else { selected = await field.nextThread(isCurrent: isCurrent) }
                    guard let selected else {
                        completion("history", .t3Unavailable); return
                    }
                    onNewField?(selected)
                    completion("switched", nil)
                }
                return
            case .pinThread, .settleThread, .rename:
                Task { @MainActor in
                    func stillOnThread() -> Bool {
                        isCurrent() && (windowThread?.isCurrent(in: target) ?? (originalField?.isFocused() == true))
                    }
                    guard stillOnThread(), let link = windowThread?.link ?? originalField?.document.flatMap(FocusedInput.threadLink) else {
                        completion("history", .t3Unavailable); return
                    }
                    do {
                        try await T3Server.shared.apply(action, thread: link.lastPathComponent, environment: link.pathComponents[1], isCurrent: stillOnThread)
                        if case .rename(let title) = action, windowThread == nil, let field = originalField {
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
                            returnToField: nil, needsField: true, onWait: nil, isCurrent: isCurrent, completion: completion)
                }
                return
            }
            func keypress(_ key: CGKeyCode, _ flags: CGEventFlags) -> Bool {
                guard let target else { return false }
                return post(key, flags: flags, to: target.processIdentifier)
            }
            if case .paste = action {
                send(action.text, shouldPressEnter: shouldPressEnter, check: check, isCurrent: isCurrent, post: keypress,
                     afterPaste: { DispatchQueue.main.asyncAfter(deadline: .now() + 0.5, execute: $0) }) { delivery, reason in
                    if delivery == "history", reason == .focusChanged, let onWait { retry(onWait) }
                    else { completion(delivery, reason) }
                }
            }
        }
        func retry(_ onWait: (Timer) -> Void) {
            var nextRestore = ProcessInfo.processInfo.systemUptime + 1
            var expectedTab: AXUIElement?
            let timer = Timer(timeInterval: 0.35, repeats: true) { timer in
                guard isCurrent() else { timer.invalidate(); return }
                if check() == .focusChanged {
                    if let field = returnToField {
                        let now = ProcessInfo.processInfo.systemUptime
                        if now >= nextRestore {
                            field.restoreFocus()
                            nextRestore = now + 1
                        }
                        if let expectedTab, SpeechAX.value(expectedTab, kAXSelectedAttribute) as? Bool != true { return }
                        if let next = field.stepTowardCmuxTab() { expectedTab = next }
                        else { field.focusReturnedField() }
                    }
                    return
                }
                timer.invalidate()
                paste()
            }
            timer.tolerance = 0.05
            RunLoop.main.add(timer, forMode: .common)
            onWait(timer)
            if timer.isValid, isCurrent() { returnToField?.restoreFocus() }
        }
        if windowThread == nil, let onWait, check() == .focusChanged {
            retry(onWait)
        } else { paste() }
    }

    // Only one temporary clipboard owner at a time (including repeated Hyper+V).
    private static var pasting = false

    static func send(_ text: String, shouldPressEnter: @escaping () -> Bool, check: @escaping () -> PasteBlockReason?, isCurrent: @escaping () -> Bool,
                     board: NSPasteboard = .general, post: @escaping (CGKeyCode, CGEventFlags) -> Bool, afterPaste: (@escaping () -> Void) -> Void,
                     completion: @escaping (String, PasteBlockReason?) -> Void) {
        if let reason = check() { completion("history", reason); return }
        guard !pasting else { completion("history", .eventUnavailable); return }
        let originalCount = board.changeCount
        var saved: [NSPasteboardItem] = []
        for item in board.pasteboardItems ?? [] {
            let copy = NSPasteboardItem()
            for type in item.types {
                // Materialize promised data before taking ownership; never lose an unreadable item.
                guard let data = item.data(forType: type), copy.setData(data, forType: type) else {
                    completion("history", .eventUnavailable); return
                }
            }
            saved.append(copy)
        }
        guard board.changeCount == originalCount else { completion("history", .eventUnavailable); return }
        let temporary = NSPasteboardItem()
        temporary.setData(Data(), forType: NSPasteboard.PasteboardType("org.nspasteboard.TransientType"))
        temporary.setString(text, forType: .string)
        pasting = true
        board.clearContents()
        let written = board.writeObjects([temporary])
        let ownedCount = board.changeCount
        func restore() {
            if board.changeCount == ownedCount {
                board.clearContents()
                if !saved.isEmpty { board.writeObjects(saved) }
            }
            pasting = false
        }
        guard written else {
            restore(); completion("history", .eventUnavailable); return
        }
        guard isCurrent() else { restore(); completion("history", nil); return }
        if let reason = check() { restore(); completion("history", reason); return }
        guard post(9, .maskCommand) else {
            restore(); completion("history", .eventUnavailable); return
        }
        // Keep the text available while the target handles Command-V, even in paste-only mode.
        afterPaste {
            restore()
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
        if key == 9 && flags == .maskCommand { log.notice("Posted Command-V to target pid=\(pid)") }
        return true
    }
}
