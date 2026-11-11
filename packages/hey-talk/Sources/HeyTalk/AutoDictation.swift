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
        if arguments.isEmpty || arguments == ["on"] || arguments == ["off"] || arguments == ["toggle"] { return }
        guard arguments.count == 2, ["add", "remove"].contains(arguments[0]),
              arguments[1].range(of: "^[A-Za-z0-9-]+(\\.[A-Za-z0-9-]+)+$", options: .regularExpression) != nil else {
            throw TalkError("Use auto [on|off|toggle|add BUNDLE_ID|remove BUNDLE_ID]")
        }
    }
    func command(_ arguments: [String]) throws {
        try Self.validate(arguments)
        guard let action = arguments.first else { return }
        if action == "toggle" { enabled.toggle(); return }
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

/// T3 stays pinned from the start; other Auto inputs follow focus until finishing.
/// A brief gap between inputs must not discard the ongoing utterance.
struct AutoRecording<T: Equatable> {
    static var focusGrace: TimeInterval { 2 }
    private(set) var target: T
    private(set) var deliveryValid = true
    let followsFocus: Bool
    private var finished = false
    private var focusMissingSince: TimeInterval?

    init(target: T, followsFocus: Bool) { self.target = target; self.followsFocus = followsFocus }
    mutating func focusChanged(_ input: T?, at time: TimeInterval) {
        if finished {
            if followsFocus && input != target { deliveryValid = false }
        } else if let input, followsFocus || input == target {
            target = input; focusMissingSince = nil
        } else if focusMissingSince == nil { focusMissingSince = time }
    }
    mutating func finish(in input: T?) {
        if followsFocus, let input { target = input }; finished = true; focusMissingSince = nil
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

    let composerLabel: String?
    var cmuxTab: AXUIElement?

    var returnInstruction: String {
        if app.bundleIdentifier == "com.t3tools.t3code", let label = composerLabel, label.hasPrefix("Message ") { return "Focus \(label.dropFirst(8)) in T3" }
        return "Focus the original field in \(app.localizedName ?? "the original app")"
    }

    static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.mismatch(rhs) == nil
    }
    func mismatch(_ other: Self?) -> String? {
        guard let other else { return "no_editable_field" }
        guard app.processIdentifier == other.app.processIdentifier else { return "app_changed" }
        guard Self.equal(window, other.window) else { return "window_changed" }
        guard document == other.document else { return "document_changed" }
        guard composerLabel == other.composerLabel else { return "composer_changed" }
        // T3 and Slack remount editors when returning to a conversation.
        if CFEqual(element, other.element) { return nil }
        guard window != nil, composerLabel != nil, let document,
              document.hasPrefix("slack://channel?") || document.hasPrefix("t3code://") else { return "element_changed" }
        return nil
    }
    private static func equal(_ lhs: AXUIElement?, _ rhs: AXUIElement?) -> Bool {
        if let lhs, let rhs { return CFEqual(lhs, rhs) }
        return lhs == nil && rhs == nil
    }
    static func isEligible(bundleID: String, role: String, subrole: String?, enabled: Bool, editable: Bool, label: String, forAuto: Bool) -> Bool {
        guard enabled, editable, subrole != "AXSecureTextField", ["AXTextField", "AXTextArea"].contains(role) else { return false }
        // T3's search, rename, and settings fields must not open the microphone.
        return !forAuto || bundleID != "com.t3tools.t3code" || (role == "AXTextArea" && (label == "Message" || label.hasPrefix("Message ")))
    }
    // Nonactivating panels can own the keyboard while their host app is in the background.
    static var keyboardApp: NSRunningApplication? {
        guard AXIsProcessTrusted() else { return NSWorkspace.shared.frontmostApplication }
        guard let app = axElement(AXUIElementCreateSystemWide(), kAXFocusedApplicationAttribute) else { return nil }
        var pid: pid_t = 0
        guard AXUIElementGetPid(app, &pid) == .success else { return nil }
        return NSRunningApplication(processIdentifier: pid)
    }
    static func read(app: NSRunningApplication, forAuto: Bool = true) -> Self? {
        guard AXIsProcessTrusted(), let bundleID = app.bundleIdentifier else { return nil }
        let application = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(application, 0.1)
        if !forAuto && ["com.t3tools.t3code", "com.tinyspeck.slackmacgap"].contains(bundleID) {
            AXUIElementSetAttributeValue(application, "AXManualAccessibility" as CFString, kCFBooleanTrue)
        }
        guard let field = axElement(application, kAXFocusedUIElementAttribute) else { return nil }
        return identify(field, app: app, forAuto: forAuto)
    }
    static func identify(_ field: AXUIElement, app: NSRunningApplication, forAuto: Bool = false) -> Self? {
        guard let bundleID = app.bundleIdentifier else { return nil }
        AXUIElementSetMessagingTimeout(field, 0.1)
        let role = value(field, kAXRoleAttribute) as? String ?? ""
        let label = value(field, kAXDescriptionAttribute) as? String ?? ""
        // Editable NSTextViews may omit AXEnabled entirely.
        var enabled: CFTypeRef?
        let enabledStatus = AXUIElementCopyAttributeValue(field, kAXEnabledAttribute as CFString, &enabled)
        var editable: DarwinBoolean = false
        guard AXUIElementIsAttributeSettable(field, kAXValueAttribute as CFString, &editable) == .success,
              isEligible(bundleID: bundleID, role: role,
                         subrole: value(field, kAXSubroleAttribute) as? String,
                         enabled: enabled as? Bool ?? (!forAuto && enabledStatus == .attributeUnsupported), editable: editable.boolValue,
                         label: label, forAuto: forAuto) else { return nil }
        let classes = value(field, "AXDOMClassList") as? [String] ?? []
        let slack = bundleID == "com.tinyspeck.slackmacgap" && classes.contains("ql-editor")
        var ancestor: AXUIElement? = field, document: String?, threadInput: String?
        var threadReply = false, mainInput = false, primaryPane = false
        var slackPanel: AXUIElement?
        for _ in 0..<48 {
            guard let current = ancestor else { break }
            if slack {
                if let id = value(current, "AXDOMIdentifier") as? String, id.hasSuffix("-thread-list-Thread_input") { threadInput = id }
                let classes = value(current, "AXDOMClassList") as? [String] ?? []
                if classes.contains("p-threads_footer__input") { threadReply = true }
                if classes.contains("p-workspace__input") { mainInput = true }
                if classes.contains("p-view_contents--primary") { primaryPane = true }
                if classes.contains("p-client_workspace__tabpanel") { slackPanel = current }
            }
            let url = value(current, kAXURLAttribute)
            if let url = url as? URL { document = url.absoluteString; break }
            if let url = url as? String { document = url; break }
            ancestor = axElement(current, kAXParentAttribute)
        }
        let window = axElement(field, kAXWindowAttribute)
        let slackComposer = slack && (threadReply || threadInput != nil || (mainInput && primaryPane))
        if slackComposer {
            guard let route = document, !threadReply || threadInput != nil,
                  let destination = slackDestination(route, threadInput: threadInput, selectedDM: selectedSlackDM(in: slackPanel)),
                  destination != route || (window != nil && !label.isEmpty) else { return nil }
            document = destination
        }
        let t3 = bundleID == "com.t3tools.t3code"
        guard !t3 || (window != nil && document?.isEmpty == false) else { return nil }
        let composer = slack || (t3 && role == "AXTextArea" && classes.contains("composer-tiptap"))
        return Self(app: app, element: field, window: window, document: document, composerLabel: composer && !label.isEmpty ? label : nil)
    }
    // Look only at the visible DM rows beside this composer, never at message history.
    private static func selectedSlackDM(in panel: AXUIElement?) -> String? {
        guard var node = panel else { return nil }
        func children(_ node: AXUIElement) -> [AXUIElement] {
            value(node, kAXChildrenAttribute) as? [AXUIElement] ?? []
        }
        func hasClass(_ node: AXUIElement, _ name: String) -> Bool {
            (value(node, "AXDOMClassList") as? [String])?.contains(name) == true
        }
        for name in ["p-view_contents--sidebar", "p-recent_page", "c-scrollbar__hider", "c-virtual_list__scroll_container"] {
            guard let child = children(node).first(where: { hasClass($0, name) }) else { return nil }
            node = child
        }
        var selected: String?
        let rows = children(node)
        guard rows.count <= 100 else { return nil }
        for row in rows {
            guard children(row).contains(where: { container in
                children(container).contains { hasClass($0, "p-activity_ia4_page__item--selected--dm") }
            }) else { continue }
            guard selected == nil, let id = value(row, "AXDOMIdentifier") as? String else { return nil }
            selected = id
        }
        return selected
    }

    static func slackDestination(_ route: String, threadInput: String?, selectedDM: @autoclosure () -> String? = nil) -> String? {
        guard let url = URL(string: route), url.scheme == "https", url.host == "app.slack.com" else { return nil }
        let path = url.pathComponents
        guard path.count >= 4, path[1] == "client", path[2].wholeMatch(of: #/^[TE][A-Z0-9]+$/#) != nil else { return nil }
        let prefix = "slack://channel?team=\(path[2])&id="
        if let threadInput {
            guard let match = threadInput.wholeMatch(of: #/^([CDG][A-Z0-9]+)-([0-9]{10}\.[0-9]{6})-thread-list-Thread_input$/#) else { return nil }
            return prefix + match.1 + "&thread_ts=" + match.2
        }
        if path.count == 4, path[3] == "dms" {
            guard let id = selectedDM(), id.wholeMatch(of: #/^[CDG][A-Z0-9]+$/#) != nil else { return route }
            return prefix + id
        }
        guard path[3].wholeMatch(of: #/^[CDG][A-Z0-9]+$/#) != nil else { return nil }
        return prefix + path[3]
    }
    static func threadLink(_ document: String) -> URL? {
        let prefix = "t3code://app/#/"
        guard document.hasPrefix(prefix) else { return nil }
        let route = document.dropFirst(prefix.count)
        let ids = route.split(separator: "/", omittingEmptySubsequences: false)
        guard ids.count == 2, ids.allSatisfy({ $0.count == 36 && UUID(uuidString: String($0)) != nil }) else { return nil }
        return URL(string: "t3code://threads/" + route)
    }
    static func composerLink(_ document: String) -> URL? {
        let prefix = "t3code://app/#/draft/"
        guard document.hasPrefix(prefix) else { return threadLink(document) }
        let id = document.dropFirst(prefix.count)
        guard id.count == 36, UUID(uuidString: String(id)) != nil else { return nil }
        return URL(string: "t3code://threads/draft/" + id)
    }
    func restoreFocus() {
        guard let window, Self.value(window, kAXRoleAttribute) as? String == kAXWindowRole else {
            Paste.log.notice("Return: original window unavailable")
            return
        }
        if !app.isActive && !app.activate(options: []), let bundle = app.bundleURL {
            NSWorkspace.shared.openApplication(at: bundle, configuration: .init())
        }
        AXUIElementPerformAction(window, kAXRaiseAction as CFString)
        if cmuxTab.map({ Self.value($0, kAXSelectedAttribute) as? Bool == true }) ?? true, Self.identify(element, app: app) == self,
           AXUIElementSetAttributeValue(element, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success, isFocused() { return }
        guard composerLabel != nil, let document, let bundle = app.bundleURL else { return }
        let link = app.bundleIdentifier == "com.tinyspeck.slackmacgap" && document.hasPrefix("slack://channel?")
            ? URL(string: document) : Self.composerLink(document)
        guard let link else { Paste.log.notice("Return: no link for original composer"); return }
        Paste.log.notice("Return: requesting original composer")
        NSWorkspace.shared.open([link], withApplicationAt: bundle, configuration: .init()) { _, error in
            if let error { Paste.log.notice("Return: composer open failed (code \((error as NSError).code))") }
        }
    }

    // Search native containers only. Chromium can return its editors directly without reading chat history.
    private func windowElements(_ visit: (AXUIElement, String) -> Bool) {
        guard let window else { return }
        var pending = [window]
        let deadline = ProcessInfo.processInfo.systemUptime + 0.12
        for _ in 0..<160 {
            guard ProcessInfo.processInfo.systemUptime < deadline, let node = pending.popLast() else { return }
            guard let role = Self.value(node, kAXRoleAttribute) as? String else { continue }
            if !visit(node, role) { continue }
            pending.append(contentsOf: Self.value(node, kAXChildrenAttribute) as? [AXUIElement] ?? [])
        }
    }

    mutating func rememberCmuxTab() {
        guard app.bundleIdentifier == "com.cmuxterm.app", Self.value(element, kAXHelpAttribute) as? String == "Terminal content area",
              let field = SpeechAX.frame(element) else { return }
        var tabs: [AXUIElement] = []
        windowElements { node, role in
            if role == kAXButtonRole, Self.value(node, kAXSelectedAttribute) as? Bool == true,
               let box = SpeechAX.frame(node), field.minX <= box.midX, box.midX <= field.maxX,
               abs(box.maxY - field.minY) < 32 { tabs.append(node) }
            return !["AXTable", "AXWebArea", "AXButton", "AXTextArea"].contains(role)
        }
        cmuxTab = tabs.count == 1 ? tabs[0] : nil
    }

    func stepTowardCmuxTab() -> AXUIElement? {
        guard app.isActive, let tab = cmuxTab, Self.value(tab, kAXSelectedAttribute) as? Bool != true,
              let window, Self.equal(Self.axElement(tab, kAXWindowAttribute), window),
              Self.equal(Self.axElement(AXUIElementCreateApplication(app.processIdentifier), kAXFocusedWindowAttribute), window) else { return nil }
        // Navigate through cmux's own View menu. AXPress on its tab buttons
        // reports success without changing tabs.
        guard let strip = Self.axElement(tab, kAXParentAttribute),
              let children = Self.value(strip, kAXChildrenAttribute) as? [AXUIElement] else { return nil }
        let tabs = children.filter { Self.value($0, kAXRoleAttribute) as? String == kAXButtonRole }
        guard let target = tabs.firstIndex(where: { CFEqual($0, tab) }),
              let current = tabs.firstIndex(where: { Self.value($0, kAXSelectedAttribute) as? Bool == true }),
              target != current, var focused = Self.read(app: app, forAuto: false) else { return nil }
        focused.rememberCmuxTab()
        guard Self.equal(focused.cmuxTab, tabs[current]) else { return nil }
        let forward = (target - current + tabs.count) % tabs.count
        let step = forward <= tabs.count - forward ? 1 : -1
        let title = step == 1 ? "Next Surface" : "Previous Surface"
        let application = AXUIElementCreateApplication(app.processIdentifier)
        guard let bar = Self.axElement(application, kAXMenuBarAttribute),
              let menus = Self.value(bar, kAXChildrenAttribute) as? [AXUIElement],
              let view = menus.first(where: { Self.value($0, kAXTitleAttribute) as? String == "View" }),
              let menu = (Self.value(view, kAXChildrenAttribute) as? [AXUIElement])?.first,
              let items = Self.value(menu, kAXChildrenAttribute) as? [AXUIElement],
              let action = items.first(where: { Self.value($0, kAXTitleAttribute) as? String == title }) else { return nil }
        guard AXUIElementPerformAction(action, kAXPressAction as CFString) == .success else { return nil }
        return tabs[(current + step + tabs.count) % tabs.count]
    }

    func focusReturnedField() {
        guard app.isActive, let window,
              Self.equal(Self.axElement(AXUIElementCreateApplication(app.processIdentifier), kAXFocusedWindowAttribute), window) else { return }
        if let cmuxTab, Self.value(cmuxTab, kAXSelectedAttribute) as? Bool != true { return }
        if Self.identify(element, app: app) == self,
           AXUIElementSetAttributeValue(element, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success { return }
        guard composerLabel != nil else { return }
        guard let editor = remountedEditor() else { return }
        AXUIElementSetAttributeValue(editor, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    }

    func remountedEditor() -> AXUIElement? {
        var matches: [AXUIElement] = []
        windowElements { node, role in
            guard role == "AXWebArea" else { return true }
            let query = ["AXSearchKey": "AXTextFieldSearchKey", "AXResultsLimit": 64] as CFDictionary
            var result: CFTypeRef?
            if AXUIElementCopyParameterizedAttributeValue(node, "AXUIElementsForSearchPredicate" as CFString, query, &result) == .success {
                matches += (result as? [AXUIElement] ?? []).filter { Self.identify($0, app: app) == self }
            }
            return false
        }
        return matches.count == 1 ? matches[0] : nil
    }
    func isFocused() -> Bool {
        guard Self.keyboardApp?.processIdentifier == app.processIdentifier else { return false }
        return Self.read(app: app, forAuto: false) == self
    }
    static func current(bundleIDs: [String]) -> Self? {
        guard let app = keyboardApp, bundleIDs.contains(app.bundleIdentifier ?? "") else { return nil }
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
        guard !suspended, AXIsProcessTrusted(), let app = FocusedInput.keyboardApp,
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
