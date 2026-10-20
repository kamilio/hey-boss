import AppKit

enum AutoSubmitKeyAction { case pass, consume, submit }
struct AutoSubmitKeyState {
    private var held: Set<Int> = []
    var passNextEnter = false
    mutating func handle(key: Int, down: Bool, modified: Bool, repeated: Bool, capture: Bool) -> AutoSubmitKeyAction {
        guard key == 36 || key == 76 else { return .pass }
        if held.contains(key) {
            if !down { held.remove(key) }
            return .consume
        }
        guard down, !modified, !repeated, capture else { return .pass }
        if passNextEnter { passNextEnter = false; return .pass }
        held.insert(key)
        return .submit
    }
}

/// Unlike a global Return hotkey, this passes through keys in every other field.
final class AutoSubmitKey {
    var shouldCapture: () -> Bool = { false }
    var onSubmit: (() -> Void)?
    var onInterruption: (() -> Void)?
    private var tap: CFMachPort?
    private var source: CFRunLoopSource?
    private var state = AutoSubmitKeyState()
    var ready: Bool { tap.map { CGEvent.tapIsEnabled(tap: $0) } ?? false }
    var passNextEnter: Bool {
        get { state.passNextEnter }
        set { state.passNextEnter = newValue }
    }

    @discardableResult func start() -> Bool {
        if ready { return true }
        stop()
        guard AXIsProcessTrusted(), let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap,
            options: .defaultTap, eventsOfInterest: (1 << CGEventType.keyDown.rawValue) | (1 << CGEventType.keyUp.rawValue),
            callback: { _, type, event, context in
                guard let context else { return Unmanaged.passUnretained(event) }
                return Unmanaged<AutoSubmitKey>.fromOpaque(context).takeUnretainedValue().handle(type, event)
            }, userInfo: Unmanaged.passUnretained(self).toOpaque()) else { return false }
        self.tap = tap
        let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
        self.source = source
        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)
        return ready
    }
    func stop() {
        if let tap { CGEvent.tapEnable(tap: tap, enable: false); CFMachPortInvalidate(tap) }
        if let source { CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes) }
        tap = nil; source = nil; state = AutoSubmitKeyState()
    }
    func handle(_ type: CGEventType, _ event: CGEvent) -> Unmanaged<CGEvent>? {
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            onInterruption?()
            if let tap { CGEvent.tapEnable(tap: tap, enable: true) }
            return Unmanaged.passUnretained(event)
        }
        // The Return generated after pasting must reach the app, never stop itself.
        guard event.getIntegerValueField(.eventSourceUserData) != Paste.eventTag else { return Unmanaged.passUnretained(event) }
        let key = Int(event.getIntegerValueField(.keyboardEventKeycode))
        guard key == 36 || key == 76 else { return Unmanaged.passUnretained(event) }
        let modified = !event.flags.intersection([.maskShift, .maskControl, .maskAlternate, .maskCommand]).isEmpty
        let repeated = event.getIntegerValueField(.keyboardEventAutorepeat) != 0
        let action = state.handle(key: key, down: type == .keyDown, modified: modified,
                                 repeated: repeated,
                                 capture: type == .keyDown && !modified && !repeated && shouldCapture())
        switch action {
        case .pass: return Unmanaged.passUnretained(event)
        case .consume: return nil
        case .submit: onSubmit?(); return nil
        }
    }
    deinit { stop() }
}
