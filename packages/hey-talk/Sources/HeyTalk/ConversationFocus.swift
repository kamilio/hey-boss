import AppKit
import TalkCore

struct ConversationFocus {
    let id = UUID().uuidString
    let app: NSRunningApplication
    let window: AXUIElement?
    var field: FocusedInput?
    let evidence: [String: String]
    let visualAllowed: Bool

    init?(app: NSRunningApplication? = FocusedInput.keyboardApp) {
        guard let app, !["local.hey-talk", "local.hey-boss.desktop"].contains(app.bundleIdentifier) else { return nil }
        self.app = app
        window = ConversationContext.focusedWindow(app)
        let root = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(root, 0.1)
        let node = AXIsProcessTrusted() ? SpeechAX.element(root, "AXFocusedUIElement") : nil
        let secure = node.map { SpeechAX.string($0, "AXSubrole") == "AXSecureTextField" } ?? false
        visualAllowed = node != nil && !secure
        field = visualAllowed ? node.flatMap { FocusedInput.identify($0, app: app) } : nil
        field?.rememberCmuxTab()
        var facts = ["app": app.localizedName ?? "unavailable", "bundle": app.bundleIdentifier ?? "unavailable",
                     "window": "unavailable", "field": "unavailable"]
        if !AXIsProcessTrusted() { facts["unavailable_reason"] = Self.permission }
        if secure { facts["field"] = "protected; omitted" }
        if visualAllowed, let window {
            let title = SpeechAX.string(window, "AXTitle")
            if !title.isEmpty { facts["window"] = String(title.prefix(300)) }
        }
        if visualAllowed, let node {
            let role = SpeechAX.string(node, "AXRole")
            let label = SpeechAX.string(node, "AXDescription")
            if !role.isEmpty { facts["field"] = String(([role, label].filter { !$0.isEmpty }).joined(separator: ": ").prefix(300)) }
        }
        evidence = facts
    }

    var canInspectWindow: Bool {
        guard visualAllowed, AXIsProcessTrusted(), !app.isTerminated, let window,
              ConversationContext.focusedWindow(app).map({ CFEqual($0, window) }) == true,
              let node = SpeechAX.element(AXUIElementCreateApplication(app.processIdentifier), "AXFocusedUIElement"),
              SpeechAX.string(node, "AXSubrole") != "AXSecureTextField" else { return false }
        return true
    }

    static let permission = "Allow Hey Talk in System Settings → Privacy & Security → Accessibility, then try again."

    @MainActor func paste(_ text: String, current: () -> Bool) async throws {
        guard AXIsProcessTrusted() else { throw TalkError(Self.permission) }
        guard current(), let field, !app.isTerminated else { throw TalkError("The original editable field is unavailable.") }
        if !field.isFocused() {
            field.restoreFocus()
            for _ in 0..<20 {
                guard current() else { throw TalkError("The conversation target changed.") }
                if field.isFocused() { break }
                try await Task.sleep(nanoseconds: 50_000_000)
            }
        }
        guard current(), field.isFocused() else { throw TalkError("Could not return to the original field. Nothing was pasted.") }
        try await withCheckedThrowingContinuation { (done: CheckedContinuation<Void, Error>) in
            Paste.deliver(.paste(text), target: app, originalField: field, shouldPressEnter: { false },
                          returnToField: nil, needsField: true, onWait: nil, isCurrent: { true }) { delivery, reason in
                if delivery == "pasted", reason == nil { done.resume() }
                else { done.resume(throwing: TalkError(reason == .accessibility ? Self.permission : reason?.message() ?? "Could not paste.")) }
            }
        }
    }
}
