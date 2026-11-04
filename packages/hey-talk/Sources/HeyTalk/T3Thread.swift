import AppKit

extension FocusedInput {
    var canUseThreadActions: Bool {
        app.bundleIdentifier == "com.t3tools.t3code" && composerLabel != nil &&
            document.flatMap(Self.threadLink) != nil
    }

    private var webArea: AXUIElement? {
        var node: AXUIElement? = element
        while let current = node, SpeechAX.string(current, "AXRole") != "AXWebArea" {
            node = SpeechAX.element(current, "AXParent")
        }
        return node
    }

    private func search(_ root: AXUIElement, _ key: String, text: String? = nil) -> [AXUIElement] {
        var query: [String: Any] = ["AXSearchKey": key, "AXResultsLimit": 64]
        query["AXSearchText"] = text
        return SpeechAX.parameter(root, "AXUIElementsForSearchPredicate", query as CFDictionary) as? [AXUIElement] ?? []
    }

    func focusedThreadTitle() -> FocusedInput? {
        guard canUseThreadActions, app.isActive, let field = Self.read(app: app, forAuto: false),
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
        guard canUseThreadActions, isCurrent(), app.isActive, let composer = Self.read(app: app, forAuto: false), composer == self else { return false }
        guard let area = composer.webArea else { return false }
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

extension FocusedInput {
    var projectButton: (name: String, button: AXUIElement)? {
        guard let area = webArea, let breadcrumb = search(area, "AXLandmarkSearchKey").first(where: {
            SpeechAX.string($0, "AXDescription") == "Thread breadcrumb"
        }) else { return nil }
        let prefix = "New thread in "
        let buttons = search(breadcrumb, "AXButtonSearchKey", text: prefix).filter { SpeechAX.string($0, "AXDescription").hasPrefix(prefix) }
        guard buttons.count == 1 else { return nil }
        return (String(SpeechAX.string(buttons[0], "AXDescription").dropFirst(prefix.count)), buttons[0])
    }

    @MainActor
    func openNewThread(project: String?, isCurrent: () -> Bool) async -> FocusedInput? {
        guard canUseThreadActions, isCurrent(), isFocused(), let area = webArea, let window else { return nil }
        func press(_ item: AXUIElement) -> Bool {
            AXUIElementPerformAction(item, "AXPress" as CFString) == .success
        }
        func open(_ button: AXUIElement, in name: String) async -> FocusedInput? {
            guard press(button) else { return nil }
            for _ in 0..<30 {
                try? await Task.sleep(nanoseconds: 40_000_000)
                guard isCurrent(), app.isActive else { return nil }
                if let draft = Self.read(app: app), let active = draft.window, CFEqual(window, active),
                   draft.document?.hasPrefix("t3code://app/#/draft/") == true,
                   let selected = draft.projectButton?.name, Self.sameProject(selected, name) { return draft }
            }
            return nil
        }
        if let current = projectButton, project.map({ Self.sameProject(current.name, $0) }) ?? true {
            guard isCurrent(), isFocused() else { return nil }
            return await open(current.button, in: current.name)
        }
        guard let project else { return nil }

        // Open the command palette. The generic New thread button can silently
        // choose the current project when the sidebar has a project filter.
        guard isCurrent(), isFocused(), Paste.post(40, flags: .maskCommand, to: app.processIdentifier) else { return nil }
        let application = AXUIElementCreateApplication(app.processIdentifier)
        func focusedPicker() -> AXUIElement? {
            guard isCurrent(), app.isActive, let activeWindow = SpeechAX.element(application, "AXFocusedWindow"), CFEqual(window, activeWindow),
                  (SpeechAX.value(area, "AXURL") as? URL)?.absoluteString == document else { return nil }
            var node = SpeechAX.element(application, "AXFocusedUIElement")
            while let current = node {
                if SpeechAX.string(current, "AXSubrole") == "AXApplicationDialog" {
                    return SpeechAX.string(current, "AXDescription") == "Command palette" ? current : nil
                }
                node = SpeechAX.element(current, "AXParent")
            }
            return nil
        }
        var openedProjects = false
        for _ in 0..<30 {
            try? await Task.sleep(nanoseconds: 40_000_000)
            guard isCurrent(), app.isActive else { return nil }
            guard let picker = focusedPicker() else { continue }
            if !openedProjects {
                let actions = search(picker, "AXAnyTypeSearchKey", text: "New thread in...").filter { SpeechAX.string($0, "AXValue") == "New thread in..." }
                guard actions.count == 1, let active = focusedPicker(), CFEqual(picker, active), press(actions[0]) else { return nil }
                openedProjects = true
                continue
            }
            // Project options expose their display name as the first text child;
            // the rest is the environment, directory, and optional shortcut.
            let headings = search(picker, "AXAnyTypeSearchKey", text: "Projects").filter { SpeechAX.string($0, "AXValue") == "Projects" }
            guard headings.count == 1, let header = SpeechAX.element(headings[0], "AXParent"),
                  let group = SpeechAX.element(header, "AXParent"),
                  let items = SpeechAX.value(group, "AXChildren") as? [AXUIElement] else { return nil }
            let rows = items.compactMap { (item: AXUIElement) -> AXUIElement? in
                guard let row = (SpeechAX.value(item, "AXChildren") as? [AXUIElement])?.first,
                      let name = (SpeechAX.value(row, "AXChildren") as? [AXUIElement])?.first,
                      SpeechAX.string(name, "AXRole") == "AXStaticText",
                      Self.sameProject(SpeechAX.string(name, "AXValue"), project) else { return nil }
                return row
            }
            guard let active = focusedPicker(), CFEqual(active, picker), rows.count == 1 else { return nil }
            return await open(rows[0], in: project)
        }
        return nil
    }

    private static func sameProject(_ name: String, _ spoken: String) -> Bool {
        func words(_ value: String) -> String {
            value.lowercased().split(whereSeparator: { "-_ ".contains($0) }).joined(separator: " ")
        }
        return words(name) == words(spoken)
    }
}
