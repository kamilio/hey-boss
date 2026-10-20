import AppKit
import TalkCore

struct AutoDictationSettings {
    private let defaults: UserDefaults
    init(defaults: UserDefaults = .standard) { self.defaults = defaults }
    var enabled: Bool {
        get { defaults.bool(forKey: "autoDictation") }
        nonmutating set { defaults.set(newValue, forKey: "autoDictation") }
    }
    var bundleIDs: [String] { (defaults.stringArray(forKey: "autoDictationApps") ?? ["com.t3tools.t3code"]).sorted() }
    var json: [String: Any] { ["enabled": enabled, "apps": bundleIDs] }

    static func validate(_ arguments: [String]) throws {
        if arguments.isEmpty || arguments == ["on"] || arguments == ["off"] { return }
        guard arguments.count == 2, ["add", "remove"].contains(arguments[0]),
              arguments[1].range(of: "^[A-Za-z0-9-]+(\\.[A-Za-z0-9-]+)+$", options: .regularExpression) != nil else {
            throw TalkError("Use auto [on|off|add BUNDLE_ID|remove BUNDLE_ID]")
        }
    }
    func command(_ arguments: [String]) throws {
        try Self.validate(arguments)
        guard let action = arguments.first else { return }
        if action == "on" || action == "off" { enabled = action == "on"; return }
        var ids = Set(bundleIDs)
        if action == "add" { ids.insert(arguments[1]) } else { ids.remove(arguments[1]) }
        defaults.set(ids.sorted(), forKey: "autoDictationApps")
    }
}

/// A focus visit is consumed even when a manual recording or decode is in progress.
/// Cancellation and failure do not reopen the microphone. A successful submit
/// or an empty attempt explicitly rearms the trigger for the next utterance.
struct FocusTrigger<T: Equatable> {
    private var previous: T?
    mutating func update(_ target: T?, available: Bool) -> Bool {
        defer { previous = target }
        return target != nil && target != previous && available
    }
    mutating func reset() { previous = nil }
}

/// The destination follows focus while speaking, then freezes when finishing.
/// A brief gap between inputs must not discard the ongoing utterance.
struct AutoRecording<T: Equatable> {
    static var focusGrace: TimeInterval { 2 }
    private(set) var target: T
    private(set) var deliveryValid = true
    private var finished = false
    private var focusMissingSince: TimeInterval?

    init(target: T) { self.target = target }
    mutating func focusChanged(_ input: T?, at time: TimeInterval) {
        if finished {
            if input != target { deliveryValid = false }
        } else if let input {
            target = input; focusMissingSince = nil
        } else if focusMissingSince == nil { focusMissingSince = time }
    }
    mutating func finish(in input: T) {
        target = input; finished = true; focusMissingSince = nil
    }
    func shouldCancel(at time: TimeInterval) -> Bool {
        focusMissingSince.map { time - $0 >= Self.focusGrace } ?? false
    }
}

struct FocusedInput: Equatable {
    let app: NSRunningApplication
    let element: AXUIElement
    let window: AXUIElement?
    let document: String?

    static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.app.processIdentifier == rhs.app.processIdentifier && CFEqual(lhs.element, rhs.element)
            && equal(lhs.window, rhs.window) && lhs.document == rhs.document
    }
    private static func equal(_ lhs: AXUIElement?, _ rhs: AXUIElement?) -> Bool {
        if let lhs, let rhs { return CFEqual(lhs, rhs) }
        return lhs == nil && rhs == nil
    }
    static func isEligible(bundleID: String, role: String, subrole: String?, enabled: Bool, editable: Bool, label: String) -> Bool {
        guard enabled, editable, subrole != "AXSecureTextField", ["AXTextField", "AXTextArea"].contains(role) else { return false }
        // T3's search, rename, and settings fields must not open the microphone.
        return bundleID != "com.t3tools.t3code" || (role == "AXTextArea" && label == "Message")
    }
    static func read(app: NSRunningApplication) -> Self? {
        guard AXIsProcessTrusted(), let bundleID = app.bundleIdentifier else { return nil }
        let application = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(application, 0.1)
        guard let field = axElement(application, kAXFocusedUIElementAttribute) else { return nil }
        AXUIElementSetMessagingTimeout(field, 0.1)
        var editable: DarwinBoolean = false
        guard AXUIElementIsAttributeSettable(field, kAXValueAttribute as CFString, &editable) == .success,
              isEligible(bundleID: bundleID, role: value(field, kAXRoleAttribute) as? String ?? "",
                         subrole: value(field, kAXSubroleAttribute) as? String,
                         enabled: value(field, kAXEnabledAttribute) as? Bool ?? false, editable: editable.boolValue,
                         label: value(field, kAXDescriptionAttribute) as? String ?? "") else { return nil }
        var ancestor: AXUIElement? = field
        var document: String?
        // Electron may keep the same editor node when navigating to another thread.
        // Include the containing document URL so that cannot redirect a transcript.
        for _ in 0..<20 {
            guard let current = ancestor else { break }
            let url = value(current, kAXURLAttribute)
            if let url = url as? URL { document = url.absoluteString; break }
            if let url = url as? String { document = url; break }
            ancestor = axElement(current, kAXParentAttribute)
        }
        return Self(app: app, element: field, window: axElement(field, kAXWindowAttribute), document: document)
    }
    func isFocused() -> Bool {
        guard NSWorkspace.shared.frontmostApplication?.processIdentifier == app.processIdentifier else { return false }
        return Self.read(app: app) == self
    }
    static func current(bundleIDs: [String]) -> Self? {
        guard let app = NSWorkspace.shared.frontmostApplication, bundleIDs.contains(app.bundleIdentifier ?? "") else { return nil }
        return read(app: app)
    }
    private static func value(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
        var result: CFTypeRef?
        return AXUIElementCopyAttributeValue(element, name as CFString, &result) == .success ? result : nil
    }
    private static func axElement(_ element: AXUIElement, _ name: String) -> AXUIElement? {
        guard let result = value(element, name), CFGetTypeID(result) == AXUIElementGetTypeID() else { return nil }
        return (result as! AXUIElement)
    }
}

/// Observe focus notifications, with a low-frequency fallback for Electron document
/// navigation and apps that miss notifications. Never read text values or selections.
final class InputFocusMonitor {
    var onChange: ((FocusedInput?) -> Void)?
    var onSuspend: (() -> Void)?
    private(set) var current: FocusedInput?
    private var bundleIDs: [String] = []
    private var timer: Timer?
    private var observer: AXObserver?
    private var observedPID: pid_t?
    private var notifications: [NSObjectProtocol] = []
    private var suspended = false

    func start(bundleIDs: [String]) {
        stop()
        self.bundleIDs = bundleIDs
        let center = NSWorkspace.shared.notificationCenter
        notifications.append(center.addObserver(forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main) { [weak self] _ in self?.refresh() })
        for name in [NSWorkspace.willSleepNotification, NSWorkspace.sessionDidResignActiveNotification] {
            notifications.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                self?.suspended = true; self?.onSuspend?(); self?.refresh()
            })
        }
        for name in [NSWorkspace.didWakeNotification, NSWorkspace.sessionDidBecomeActiveNotification] {
            notifications.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                self?.suspended = false; self?.refresh()
            })
        }
        let timer = Timer(timeInterval: 0.5, repeats: true) { [weak self] _ in self?.refresh() }
        timer.tolerance = 0.1
        RunLoop.main.add(timer, forMode: .common); self.timer = timer
        refresh()
    }
    func stop() {
        timer?.invalidate(); timer = nil
        notifications.forEach { NSWorkspace.shared.notificationCenter.removeObserver($0) }; notifications = []
        detach(); bundleIDs = []; suspended = false
        publish(nil)
    }
    func refresh() {
        guard !suspended, AXIsProcessTrusted(), let app = NSWorkspace.shared.frontmostApplication,
              bundleIDs.contains(app.bundleIdentifier ?? "") else {
            detach(); publish(nil); return
        }
        if observedPID != app.processIdentifier { attach(app) }
        publish(FocusedInput.read(app: app))
    }
    private func publish(_ input: FocusedInput?) {
        guard current != input else { return }
        current = input; onChange?(input)
    }
    private func attach(_ app: NSRunningApplication) {
        detach(); observedPID = app.processIdentifier
        let element = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(element, 0.1)
        // Chromium creates its accessibility tree on demand for assistive clients.
        AXUIElementSetAttributeValue(element, "AXManualAccessibility" as CFString, kCFBooleanTrue)
        var next: AXObserver?
        guard AXObserverCreate(app.processIdentifier, { _, _, _, context in
            guard let context else { return }
            Unmanaged<InputFocusMonitor>.fromOpaque(context).takeUnretainedValue().refresh()
        }, &next) == .success, let next else { return }
        observer = next
        for notification in [kAXFocusedUIElementChangedNotification, kAXFocusedWindowChangedNotification] {
            AXObserverAddNotification(next, element, notification as CFString, Unmanaged.passUnretained(self).toOpaque())
        }
        CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(next), .commonModes)
    }
    private func detach() {
        if let observer { CFRunLoopRemoveSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(observer), .commonModes) }
        observer = nil; observedPID = nil
    }
    deinit { stop() }
}
