import AppKit

struct T3ThreadRow {
    let title: String
    let label: String
    let element: AXUIElement
    var current: Bool {
        (SpeechAX.value(element, "AXDOMClassList") as? [String] ?? []).contains("bg-sidebar-row-active")
    }
}

extension FocusedInput {
    var canUseThreadActions: Bool {
        app.bundleIdentifier == "com.t3tools.t3code" && composerLabel != nil &&
            document.flatMap(Self.threadLink) != nil
    }

    var t3ThreadID: String? { document.flatMap(Self.threadLink)?.lastPathComponent }
    var t3EnvironmentID: String? { document.flatMap(Self.threadLink)?.pathComponents.dropFirst().first }

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

    func threadRows() -> [T3ThreadRow]? {
        guard canUseThreadActions, isFocused(), let area = webArea,
              let sidebar = search(area, "AXLandmarkSearchKey").first(where: { SpeechAX.string($0, "AXDescription") == "Threads" }) else { return nil }
        let rows = search(sidebar, "AXButtonSearchKey").filter {
            (SpeechAX.value($0, "AXDOMClassList") as? [String] ?? []).contains("group/sidebar-row")
        }
        let result = rows.prefix(9).compactMap { row -> T3ThreadRow? in
            guard let text = search(row, "AXStaticTextSearchKey").first else { return nil }
            return T3ThreadRow(title: SpeechAX.string(text, "AXValue"), label: SpeechAX.string(row, "AXDescription"), element: row)
        }
        return result.count == min(rows.count, 9) ? result : nil
    }

    @MainActor
    func nextThread(isCurrent: () -> Bool) async -> FocusedInput? {
        guard isCurrent(), isFocused(), canUseThreadActions, let window,
              Paste.post(30, flags: [.maskCommand, .maskShift], to: app.processIdentifier) else { return nil }
        for _ in 0..<40 {
            try? await Task.sleep(nanoseconds: 40_000_000)
            guard isCurrent(), app.isActive else { break }
            if let field = Self.read(app: app), field.canUseThreadActions, field.document != document,
               let destination = field.window, CFEqual(window, destination) { return field }
        }
        return nil
    }

    @MainActor
    func switchThread(position: Int, listed: [T3ThreadRow]?, isCurrent: () -> Bool) async -> FocusedInput? {
        guard isCurrent(), (1...9).contains(position), let rows = threadRows(), rows.count >= position else { return nil }
        let selected = rows[position - 1]
        if let listed {
            guard listed.count >= position, CFEqual(listed[position - 1].element, selected.element),
                  listed[position - 1].title == selected.title else { return nil }
        }
        var expectedID: String?
        if listed != nil {
            guard let environment = t3EnvironmentID,
                  let threads = try? await T3Server.shared.threads(environment: environment) else { return nil }
            let matches = threads.filter { $0["title"] as? String == selected.title }
            guard matches.count == 1, let id = matches.first?["id"] as? String, UUID(uuidString: id) != nil,
                  let url = URL(string: "t3code://threads/\(environment)/\(id)"), let bundle = app.bundleURL,
                  isCurrent(), isFocused(), let fresh = threadRows(), fresh.count >= position,
                  CFEqual(fresh[position - 1].element, selected.element), fresh[position - 1].title == selected.title else { return nil }
            expectedID = id
            NSWorkspace.shared.open([url], withApplicationAt: bundle, configuration: .init(), completionHandler: nil)
        } else {
            let keys: [CGKeyCode] = [18, 19, 20, 21, 23, 22, 26, 28, 25]
            guard isCurrent(), isFocused(), Paste.post(keys[position - 1], flags: .maskCommand, to: app.processIdentifier) else { return nil }
        }
        for _ in 0..<40 {
            try? await Task.sleep(nanoseconds: 40_000_000)
            guard isCurrent(), app.isActive else { return nil }
            guard let field = Self.read(app: app), field.canUseThreadActions,
                  let window, let active = field.window, CFEqual(window, active),
                  selected.current, field.composerLabel == "Message " + selected.title,
                  expectedID == nil || field.t3ThreadID == expectedID else { continue }
            return field
        }
        return nil
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
