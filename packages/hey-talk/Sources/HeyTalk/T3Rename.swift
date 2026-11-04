import AppKit

extension FocusedInput {
    var canRenameThread: Bool {
        app.bundleIdentifier == "com.t3tools.t3code" && composerLabel != nil &&
            document.flatMap(Self.threadLink) != nil
    }

    func focusedThreadTitle() -> FocusedInput? {
        guard canRenameThread, app.isActive, let field = Self.read(app: app, forAuto: false),
              field.document == document, let window, let current = field.window, CFEqual(window, current),
              SpeechAX.string(field.element, "AXRole") == "AXTextField",
              SpeechAX.string(field.element, "AXDescription") == "Thread title" else { return nil }
        var ancestor = SpeechAX.element(field.element, "AXParent")
        while let node = ancestor {
            if SpeechAX.string(node, "AXDescription") == "Thread breadcrumb" { return field }
            ancestor = SpeechAX.element(node, "AXParent")
        }
        return nil
    }

    @MainActor
    func openThreadTitle(isCurrent: () -> Bool) async -> Bool {
        guard canRenameThread, isCurrent(), app.isActive, let composer = Self.read(app: app, forAuto: false), composer == self else { return false }
        var ancestor: AXUIElement? = composer.element
        while let node = ancestor, SpeechAX.string(node, "AXRole") != "AXWebArea" {
            ancestor = SpeechAX.element(node, "AXParent")
        }
        guard let area = ancestor else { return false }
        func search(_ root: AXUIElement, _ key: String) -> [AXUIElement] {
            let query = ["AXSearchKey": key, "AXResultsLimit": 64] as CFDictionary
            return SpeechAX.parameter(root, "AXUIElementsForSearchPredicate", query) as? [AXUIElement] ?? []
        }
        guard let breadcrumb = search(area, "AXLandmarkSearchKey").first(where: {
            SpeechAX.string($0, "AXDescription") == "Thread breadcrumb"
        }) else { return false }
        let buttons = search(breadcrumb, "AXButtonSearchKey")
        let matches = buttons.filter { SpeechAX.string($0, "AXDescription").hasPrefix("Thread actions for ") }
        guard matches.count == 1, let button = matches.first, isFocused(),
              AXUIElementPerformAction(button, "AXPress" as CFString) == .success else { return false }
        let application = AXUIElementCreateApplication(app.processIdentifier)
        for _ in 0..<40 {
            try? await Task.sleep(nanoseconds: 40_000_000)
            guard isCurrent(), app.isActive else { return false }
            guard let focused = SpeechAX.element(application, "AXFocusedUIElement"), CFEqual(button, focused),
                  let window, let menu = (SpeechAX.value(window, "AXChildren") as? [AXUIElement])?.first(where: {
                      SpeechAX.string($0, "AXRole") == "AXMenu"
                  }), let items = SpeechAX.value(menu, "AXChildren") as? [AXUIElement],
                  let rename = items.first(where: { SpeechAX.string($0, "AXTitle") == "Rename thread" }),
                  SpeechAX.value(rename, "AXEnabled") as? Bool == true else { continue }
            guard Self.identify(composer.element, app: app) == self else { return false }
            // Use the live key equivalent when one exists; current T3 exposes
            // Rename thread as a menu action with no default shortcut.
            if !SpeechAX.string(rename, "AXMenuItemCmdChar").isEmpty,
               let key = SpeechAX.value(rename, "AXMenuItemCmdVirtualKey") as? UInt16,
               let modifiers = SpeechAX.value(rename, "AXMenuItemCmdModifiers") as? Int {
                var flags: CGEventFlags = modifiers & 8 == 0 ? .maskCommand : []
                if modifiers & 1 != 0 { flags.insert(.maskShift) }
                if modifiers & 2 != 0 { flags.insert(.maskAlternate) }
                if modifiers & 4 != 0 { flags.insert(.maskControl) }
                return Paste.post(key, flags: flags, to: app.processIdentifier)
            }
            return AXUIElementPerformAction(rename, "AXPress" as CFString) == .success
        }
        return false
    }
}
