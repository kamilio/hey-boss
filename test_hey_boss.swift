import AppKit
import Foundation
import Darwin
import ImageIO
import zlib
import SQLite3
import Carbon
import WebKit

func audit() {
    // Preserve the last completed check in CI logs if an optimized precondition traps.
    setbuf(stdout, nil)
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    if let path = ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_SKILL_EDITOR"] { auditSkillEditor(path); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_IMAGES"] == "1" { auditImageNotifications(); return }
    if let state = ProcessInfo.processInfo.environment["HEY_BOSS_QUOTA_VISUAL"] { previewSubscriptionQuota(state); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_REPLAY"] == "1" { auditNotificationReplay(); return }
    if let state = ProcessInfo.processInfo.environment["HEY_BOSS_QUICK_ISSUE_VISUAL"] {
        app.setActivationPolicy(.regular)
        let suite = "hey-boss-quick-visual"
        let prefs = UserDefaults(suiteName: suite)!
        defer { prefs.removePersistentDomain(forName: suite) }
        let ui = NativeQuickIssue(preferences: prefs)
        ui.runner = { _, reply in reply(.success(Data("{\"projects\":[{\"id\":\"named:Visual\",\"name\":\"Visual\"}]}".utf8))) }
        ui.open(cli: nil); ui.current = ui.projects.first
        ui.window.delegate = nil
        ui.window.appearance = NSAppearance(named: state.contains("dark") ? .darkAqua : .aqua)
        ui.input.string = "Fix the screenshot preview @Visual"; ui.changed()
        if state.contains("images") || state.contains("saving") {
            let image = NSImage(size: NSSize(width: 240, height: 120), flipped: false) { rect in
                NSColor.systemIndigo.setFill(); rect.fill()
                NSColor.systemTeal.setFill(); NSBezierPath(roundedRect: rect.insetBy(dx: 30, dy: 25), xRadius: 12, yRadius: 12).fill(); return true
            }
            ui.images = (1...5).map { QuickIssueImage(name: "Screenshot \($0).png", data: Data(), thumbnail: image) }; ui.rebuildImages()
        }
        if state.contains("saving") { ui.saving = true; ui.context.stringValue = "Attaching image 2 of 5…" }
        if state.contains("error") { ui.error.stringValue = "Cannot read this image. Choose an image up to 10 MiB." }
        if state.contains("multiline") { ui.input.string = "Fix reconnect after sleep\n\nSteps to reproduce:\n  - Sleep the Mac\n  - Wake and reconnect 🦀"; ui.changed() }
        if state.contains("long") { ui.input.string = "Long draft\n" + String(repeating: "Keep every line visible while editing.\n", count: 40); ui.changed() }
        if state.contains("empty") { ui.input.string = "" }
        if state.contains("success") { ui.succeeded = true; ui.context.stringValue = "Created #42 in Visual" }
        if state.contains("compact") { ui.window.setContentSize(NSSize(width: 520, height: ui.window.frame.height)) }
        if state.contains("command") { ui.showShortcuts(true) }
        ui.updateEnabled(); ui.layout()
        withExtendedLifetime(ui) { app.run() }; return
    }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_NOTIFICATION_AGENTS"] == "1" { auditNotificationAgents(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_QUIET_HOURS"] == "1" { auditQuietHours(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_PAGING"] == "1" { auditPaging(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_ACTIVE_AGENTS_ONLY"] == "1" { auditActiveAgentFilter(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_WEB_INBOX_ONLY"] == "1" {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("hb-web-inbox-"+UUID().uuidString)
        try! FileManager.default.createDirectory(at:root,withIntermediateDirectories:true)
        defer { try? FileManager.default.removeItem(at:root) }
        auditWebInbox(root:root); return
    }
    if let path = ProcessInfo.processInfo.environment["HEY_BOSS_ARTIFACT_EDITOR_PREVIEW"] {
        app.setActivationPolicy(.regular)
        let launch = ArtifactLaunch(project: "named:Editor visual test", file: path)
        let editor = NativeArtifactEditor(launch: launch, journalRoot: URL(fileURLWithPath: path).deletingLastPathComponent().appendingPathComponent("drafts"))
        ArtifactEditors.shared.windows = [editor]; app.delegate = ArtifactEditors.shared
        editor.onOpen = { ArtifactEditors.shared.open($0) }
        editor.show()
        withExtendedLifetime(editor) { app.run() }
        return
    }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_DESKTOP_ACTIONS_ONLY"] == "1" { auditDesktopActions(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_ARTIFACT_EDITOR"] == "1" { auditArtifactEditor(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_HTML_READER"] == "1" { auditHTMLReader(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_ARTIFACT_LIBRARY"] == "1" { auditArtifactLibrary(); return }
    if let path = ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_POE_CORPUS"] { auditPoeMarkdownCorpus(path); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_NATIVE_READER"] == "1" {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("hb-native-reader-"+UUID().uuidString)
        try! FileManager.default.createDirectory(at:root,withIntermediateDirectories:true)
        defer { try? FileManager.default.removeItem(at:root) }
        let sample = Record(taskID:"native-sample",kind:"update",question:"# Sample\n\nText",project:"Synthetic",title:"Sample",description:"",options:[],autoclose:nil,linkURL:nil,linkLabel:nil,createdAt:0,presentedAt:nil,expiresAt:nil,status:"pending",result:nil,origin:nil)
        auditNativeMarkdownReader(sample:sample)
        auditLargeNativeMarkdownReader(sample:sample)
        auditDocumentComments(root:root,sample:sample)
        auditReviewImage(sample:sample)
        auditCommentComposer(root:root)
        auditMultilineSourceComment(root:root)
        return
    }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_NOTIFICATION_CLICKS"] == "1" { auditNotificationClicks(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_LOCAL_COMPLETION"] == "1" {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("hb-local-completion-" + UUID().uuidString)
        try! FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        auditCloseAll(root: root); return
    }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_OPTIMISTIC"] == "1" { auditOptimisticNotifications(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_ISSUES_SHORTCUT_ONLY"] == "1" { auditIssuesShortcut(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_NATIVE_QUICK_ISSUE_PREVIEW"] == "1" {
        let prefs = UserDefaults(suiteName: "hey-boss-issue66-preview")!
        defer { prefs.removePersistentDomain(forName: "hey-boss-issue66-preview") }
        let ui = NativeQuickIssue(preferences: prefs)
        let shortcut = QuickIssueShortcut { ui.open(cli: nil) }
        ui.open(cli: nil)
        withExtendedLifetime((ui, shortcut)) { app.run() }
        return
    }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_QUICK_ISSUE_ONLY"] == "1" { auditQuickIssue(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_SECRET_ONLY"] == "1" { auditSecretInput(); return }
    if let appearance = ProcessInfo.processInfo.environment["HEY_BOSS_SECRET_VISUAL"] {
        // Isolated synthetic window for interactive testing; no daemon or store.
        let data = try! JSONSerialization.data(withJSONObject: ["command":"secret", "sync":true, "project":"Synthetic UI test", "title":"Connection credentials"])
        let request = try! JSONDecoder().decode(Request.self, from: data)
        let form = SecretForm(fields: ["LOGIN", "PASSWORD"], login: true, destination: "Synthetic child process · no credentials will be used", reply: true)
        let prompt = SecretPrompt(request: request, form: form) { outcome in
            switch outcome {
            case .replied(let text): print("Synthetic reply received: \(text)")
            case .submitted: print("Synthetic credentials received")
            case .cancelled: print("Synthetic request cancelled")
            case .expired: print("Synthetic request expired")
            }
            NSApp.stop(nil)
        }
        prompt.window.appearance = NSAppearance(named: appearance == "dark" ? .darkAqua : .aqua)
        // This test window contains synthetic inputs only; production remains excluded from capture.
        prompt.window.sharingType = .readOnly
        prompt.show()
        withExtendedLifetime(prompt) { app.run() }; return
    }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_QUOTA_ONLY"] == "1" { auditSubscriptionQuota(); return }
    auditSubscriptionQuota()
    auditQuickIssue()
    auditArtifactEditor()
    auditArtifactLibrary()
    auditIssuesShortcut()
    auditSecretInput()
    auditNotificationReplay()
    auditPaging()
    if ProcessInfo.processInfo.environment["HEY_BOSS_PERFORMANCE"] == "1" { auditPerformance(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_AGENTS_ONLY"] == "1" { _ = auditAgentOverview(); return }
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("hey-boss-test-\(UUID().uuidString)")
    try! FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try! FileManager.default.removeItem(at: root) }
    let store = try! Store(root.appendingPathComponent("history.db").path)
    let ui = Interface(present: false)
    store.show = { ui.add($0) }
    store.remove = { ui.remove($0) }
    store.removeMany = { ui.remove($0) }
    ui.onComplete = { store.complete($0, $1) }
    ui.onCompleteMany = { store.complete($0) }
    ui.onDismissMany = { store.dismiss($0) }
    ui.onPresented = { store.presented($0, $1) }
    func row(_ kind: String, _ project: String, _ options: [String]) -> Record {
        Record(taskID: UUID().uuidString, kind: kind, question: "# Report\n\n[Read more](https://example.com)\n[WIP](/Users/example/My%20Project/WIP.md)\n[Relative](README.md) [Anchor](#section) [Unsupported](javascript:alert)", project: project, title: "Build complete", description: "All checks passed", options: options, autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: Date().timeIntervalSince1970, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
    }
    var sourceFixture = row("update", "Sources", [])
    precondition(sourceFixture.sourceLabel == "Source unavailable")
    sourceFixture.sourceHost = "This Mac"
    precondition(sourceFixture.isLocalSource && sourceFixture.sourceLabel == "This Mac")
    sourceFixture.sourceHost = "devbox\n"
    precondition(!sourceFixture.isLocalSource && sourceFixture.sourceLabel == "devbox")
    let restoredSource = try! JSONDecoder().decode(Record.self, from: JSONEncoder().encode(sourceFixture))
    precondition(restoredSource.sourceLabel == "devbox")
    let updates = (0..<3).map { _ in row("update", "Atlas", []) }
    var openedURLs: [URL] = []
    ui.openURL = { openedURLs.append($0) }
    for item in updates.prefix(2) { (try! store.database.save(item)); ui.add(item) }
    precondition(ui.projectGroups.isEmpty)
    (try! store.database.save(updates[2]))
    ui.add(updates[2])
    precondition(ui.projectGroups.count == 1)
    ui.expandedProjects.insert("Atlas")
    ui.layout()
    precondition(ui.cards.allSatisfy { !$0.view.isHidden })
    // AppKit glass containers settle their internal content geometry on the run loop.
    ui.stackContent.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    let readButton = ui.cards[0].link!
    let readPoint = readButton.convert(NSPoint(x: readButton.bounds.midX, y: readButton.bounds.midY), to: ui.cards[0].view.superview)
    let readHit = ui.cards[0].view.hitTest(readPoint)
    print("Read update hit target: \(String(describing: readHit))")
    precondition(readHit === readButton, "Read update must receive actual mouse hit testing")
    let rootPoint = readButton.convert(NSPoint(x: readButton.bounds.midX, y: readButton.bounds.midY), to: ui.stackContent.superview)
    let rootHit = ui.stackContent.hitTest(rootPoint)
    print("Read update root hit target: \(String(describing: rootHit))")
    precondition(rootHit === readButton, "Notification container must route clicks to Read update")
    ui.cards[0].link!.performClick(nil)
    precondition((try! store.database.get(updates[0].taskID)).status == "ok")
    precondition(ui.cards.count == 2 && ui.projectGroups.isEmpty, "Two survivors must no longer be grouped")
    precondition(ui.previews.count == 1)
    let preview = ui.previews[updates[0].taskID]!
    waitForNativeReader(preview)
    var previewLinks: [URL] = []
    preview.text.textStorage!.enumerateAttribute(.link,in:NSRange(location:0,length:preview.text.textStorage!.length)) { value,_,_ in
        if let url = value as? URL, url.scheme != nil { previewLinks.append(url) }
    }
    precondition(previewLinks.count == 2)
    precondition(preview.text.string.contains("Relative Anchor Unsupported"))
    _ = preview.textView(preview.text,clickedOnLink:previewLinks[0],at:0)
    _ = preview.textView(preview.text,clickedOnLink:previewLinks[1],at:0)
    precondition(openedURLs == [URL(string: "https://example.com")!, URL(fileURLWithPath: "/Users/example/My Project/WIP.md")])
    for preview in ui.previews.values { preview.close() }
    for project in ["frontend-web", "frontend-api"] {
        let card = Card(row("alert", project, []), open: {}, openURL: { _ in }, complete: { _, _ in })
        card.configure(grouped: false)
        let required = (card.projectLabel.stringValue as NSString).size(withAttributes: [.font: card.projectLabel.font!]).width
        precondition(card.projectLabel.frame.width >= required)
        precondition(card.header.frame.minX > card.projectLabel.frame.maxX && card.header.frame.width > 60)
    }
    let other = row("alert", "Orion", [])
    (try! store.database.save(other))
    ui.add(other)
    store.complete(Array(updates.dropFirst()).map(\.taskID))
    precondition(ui.cards.count == 1 && ui.cards[0].row.project == "Orion")
    precondition((try! store.database.get(other.taskID)).status == "pending")
    let prompt = row("prompt", "Atlas", [])
    let choice = row("approval", "Atlas", ["PDF", "Markdown"])
    for item in [prompt, choice] { (try! store.database.save(item)); ui.add(item) }
    precondition(ui.field != nil && ui.current!.taskID == prompt.taskID)
    ui.answer("Résumé α")
    precondition(ui.current!.taskID == choice.taskID)
    ui.answer("Markdown")
    precondition(ui.current == nil && ui.question.contentView == nil)
    let reopened = try! Database(root.appendingPathComponent("history.db").path)
    precondition((try! reopened.get(prompt.taskID)).result == "Résumé α")
    precondition((try! reopened.get(choice.taskID)).result == "Markdown")
    precondition((try! reopened.pending()).map(\.taskID) == [other.taskID])
    let attributed = markdown("[Read more](https://example.com)", size: 14, color: .labelColor)
    var links = 0
    attributed.enumerateAttribute(.link, in: NSRange(location: 0, length: attributed.length)) { value, _, _ in if value != nil { links += 1 } }
    precondition(links == 1)
    precondition(markdownLinkURL(URL(string: "file:///tmp/report.md")!)!.isFileURL)
    precondition(markdownLinkURL(URL(string: "//example.com/report")!) == nil)
    auditAppearance(root: root, sample: updates[0])
    auditDismissalAnimation(sample: updates[0])
    auditCloseAll(root: root)
    auditInbox(root: root, sample: updates[0])
    auditWebInbox(root:root)
    auditConnectionSettings()
    auditResilience(root: root)
    auditDesktopActions()
    auditScannerOutput(root: root)
    auditSocketDeadlines()
    auditOverviewExpansionPersistence()
    _ = auditAgentOverview()
    auditNativeMarkdownReader(sample: updates[0])
    auditDocumentComments(root: root, sample: updates[0])
    auditReviewImage(sample: updates[0])
    auditImageNotifications()
    auditCommentComposer(root: root)
    auditMultilineSourceComment(root: root)
    auditOptimisticNotifications()
    auditNotificationClicks()
    print("Passed: grouping threshold, CTA dismissal, preview with local/web/unsupported links, project isolation, queued questions, Unicode answers, durable history, Markdown links")
}


func auditIssuesShortcut() {
    let overview = AgentsOverview(present: false, cli: nil)
    var urls: [URL] = []
    overview.issuesLauncher.probe = { $0(true) }
    overview.issuesLauncher.openURL = { urls.append($0); return true }
    precondition(!overview.issuesMenuItem.menu!.items.contains { $0.title.contains("Mindmap") })
    let item = overview.issuesMenuItem
    precondition(item.keyEquivalent == "o" && item.keyEquivalentModifierMask == [.command, .control, .option, .shift])
    precondition(NSApp.sendAction(item.action!, to: item.target, from: item))
    precondition(urls.count == 1 && urls[0].absoluteString == "http://hey-boss.test/#view=issues", "Issues opens the normal list without imposing filters")
    let shortcut = IssuesShortcut { overview.showIssues() }
    withExtendedLifetime(shortcut) {
        var event: EventRef?
        precondition(CreateEvent(nil, OSType(kEventClassKeyboard), UInt32(kEventHotKeyPressed), GetCurrentEventTime(), 0, &event) == noErr)
        defer { ReleaseEvent(event) }
        for (signature, id, expected) in [(UInt32(0x4842494F), UInt32(1), 2), (UInt32(0x4842494F), UInt32(2), 2), (UInt32(0x48425149), UInt32(1), 2)] {
            var identity = EventHotKeyID(signature: signature, id: id)
            precondition(SetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), MemoryLayout<EventHotKeyID>.size, &identity) == noErr)
            _ = SendEventToEventTarget(event, GetApplicationEventTarget())
            precondition(urls.count == expected, "Only the Issues hotkey opens the browser")
            precondition(urls.last?.absoluteString == "http://hey-boss.test/#view=issues", "The global shortcut uses the same unfiltered destination as the menu")
        }
    }
    var failures: [String] = []
    overview.issuesLauncher.probe = { $0(false) }
    overview.issuesLauncher.report = { failures.append($0) }
    overview.showIssues()
    precondition(failures.count == 1 && !overview.issuesLauncher.launching && urls.count == 2)
    let launcher = IssuesLauncher()
    var starts = 0, probes = 0
    launcher.start = { _ in starts += 1 }
    launcher.openURL = { urls.append($0); return true }
    launcher.probe = { probes += 1; $0(probes > 1) }
    launcher.open(cli: "/usr/bin/true")
    precondition(starts == 1 && urls.count == 3 && !launcher.launching, "Start the missing service before opening the browser")
    var ready: ((Bool) -> Void)?
    launcher.probe = { ready = $0 }
    launcher.open(cli: nil); launcher.open(cli: nil)
    ready?(true)
    precondition(starts == 1 && urls.count == 4 && !launcher.launching, "Repeated shortcut activation must reuse the service")
    launcher.report = { failures.append($0) }
    launcher.probe = { $0(true) }
    launcher.openURL = { _ in false }
    launcher.open(cli: nil)
    precondition(failures.count == 2 && failures.last!.contains("http://hey-boss.test/"))
    print("Passed: Issues menu shortcut, global callback, browser destination, unrelated hotkeys, cold start, repeated activation and error recovery")
}

func auditQuickIssue() {
    let overview = AgentsOverview(present: false, cli: nil)
    overview.quickIssue.runner = { _, completion in completion(.success(Data("{\"projects\":[]}".utf8))) }
    overview.showQuickIssue()
    precondition(overview.quickIssue.ready)
    let item = overview.statusMenu.items.first { $0.action == #selector(AgentsOverview.showQuickIssue) }!
    precondition(item.keyEquivalent == "i" && item.keyEquivalentModifierMask == [.command, .control, .option, .shift])
    precondition(NSApp.sendAction(item.action!, to: item.target, from: item))
    auditQuickIssueMedia()
    auditNativeQuickIssue()
    if ProcessInfo.processInfo.environment["HEY_BOSS_QUICK_ISSUE_INTEGRATION"] == "1" { auditQuickIssueMediaIntegration() }
    var calls = 0
    let shortcut = QuickIssueShortcut { calls += 1 }
    withExtendedLifetime(shortcut) {
        var event: EventRef?
        precondition(CreateEvent(nil, OSType(kEventClassKeyboard), UInt32(kEventHotKeyPressed), GetCurrentEventTime(), 0, &event) == noErr)
        defer { ReleaseEvent(event) }
        var identity = EventHotKeyID(signature: 0x48425149, id: 1)
        precondition(SetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), MemoryLayout<EventHotKeyID>.size, &identity) == noErr)
        precondition(SendEventToEventTarget(event, GetApplicationEventTarget()) == noErr && calls == 1)
        identity.id = 2
        precondition(SetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), MemoryLayout<EventHotKeyID>.size, &identity) == noErr)
        _ = SendEventToEventTarget(event, GetApplicationEventTarget())
        precondition(calls == 1, "Other hotkeys must not open quick add")
    }
    print("Passed: quick-add menu, launch destination, global shortcut callback and unrelated hotkeys")
}

func auditNativeQuickIssue() {
    let projects = [QuickIssueProject(id: "github.com/kamilio/hey-boss", name: "hey-boss"), QuickIssueProject(id: "github.com/poe-platform/poe-code", name: "poe-code"), QuickIssueProject(id: "named:Design Team", name: "Design Team"), QuickIssueProject(id: "named:café", name: "café")]
    for (text, expected, destination) in [
        ("Fix reconnect", "Fix reconnect", 0), ("Fix @POE-CODE reconnect", "Fix reconnect", 1),
        ("@github.com/poe-platform/poe-code Fix", "Fix", 1), ("Fix @\"Design Team\" reconnect", "Fix reconnect", 2),
        ("Fix @'Design Team' reconnect", "Fix reconnect", 2), ("Fix @café reconnect", "Fix reconnect", 3),
        ("Email boss@poe-code.com", "Email boss@poe-code.com", 0), ("Document \\@poe-code syntax", "Document @poe-code syntax", 0),
        ("Fix @poe-code. Now", "Fix . Now", 1), ("Fix (@poe-code), reconnect", "Fix (), reconnect", 1),
        ("Fix @poe-code and @poe-code", "Fix and", 1)
    ] {
        let value = try! QuickIssueText.parse(text, projects: projects, current: projects[0])
        precondition(value.title == expected && value.project == projects[destination], text)
    }
    for text in ["@missing Fix", "@poe-code-extra Fix", "@poe-code @hey-boss Fix", "@\"Design Team Fix", "@ Fix", "@poe-code", "   ", "Fix @'Design Team'x"] {
        precondition((try? QuickIssueText.parse(text, projects: projects, current: projects[0])) == nil, text)
    }
    let duplicates = projects + [QuickIssueProject(id: "named:other", name: "poe-code")]
    precondition((try? QuickIssueText.parse("Fix @poe-code", projects: duplicates, current: projects[0])) == nil)
    precondition(QuickIssueText.completion(projects[1], projects: duplicates) == "@github.com/poe-platform/poe-code")
    precondition(QuickIssueText.suggestions(projects, query: "PO") == [projects[1]])
    precondition(QuickIssueText.suggestions(projects, query: "cafe\u{301}") == [projects[3]])
    precondition((try? QuickIssueText.parse("Fix", projects: projects, current: nil)) == nil)
    let longTitle = String(repeating: "é", count: 513)
    precondition(try! QuickIssueText.parse(longTitle, projects: projects, current: projects[0]).title == longTitle, "Pass all text to the Rust title splitter")
    let multiline = try! QuickIssueText.parse("Fix reconnect @poe-code\n\nSteps:\n  - Sleep the Mac\n  - Wake it 🦀", projects: projects, current: projects[0])
    precondition(multiline.title == "Fix reconnect", "The first line is the title; later lines must not be flattened into it")
    precondition(multiline.body == "Steps:\n  - Sleep the Mac\n  - Wake it 🦀" && multiline.project == projects[1])
    let windowsPaste = try! QuickIssueText.parse("Title\r\n\r\nParagraph\r\n\r\n    code", projects: projects, current: projects[0])
    precondition(windowsPaste.title == "Title" && windowsPaste.body == "Paragraph\n\n    code", "Pasted paragraphs and code indentation survive")
    let suite = "hey-boss-native-quick-issue-\(UUID().uuidString)"; let prefs = UserDefaults(suiteName: suite)!
    defer { prefs.removePersistentDomain(forName: suite) }
    let ui = NativeQuickIssue(present: false, preferences: prefs)
    var requests: [[String]] = []; var reply: ((Result<Data, Error>) -> Void)?
    let bootstrap = try! JSONEncoder().encode(["projects": projects])
    ui.runner = { args, completion in
        requests.append(args)
        if args.first == "projects" { completion(.success(bootstrap)) } else { reply = completion }
    }
    ui.open(cli: nil); precondition(ui.ready && ui.current == nil)
    precondition(ui.canvas.bounds.width == ui.window.frame.width && ui.canvas.bounds.height == ui.window.frame.height, "Glass content must fill the quick-add panel")
    ui.current = projects[1]; ui.input.string = "Title"; ui.changed()
    precondition(ui.mediaHint.stringValue == "⌘V Attach image", "Only the attachment hint belongs below the editor")
    let destination = ui.context.attributedStringValue
    let projectRange = (destination.string as NSString).range(of: projects[1].name)
    let destinationFont = destination.attribute(.font, at: projectRange.location, effectiveRange: nil) as! NSFont
    precondition(NSFontManager.shared.traits(of: destinationFont).contains(.boldFontMask), "Emphasize the destination project")
    precondition(ui.bottom.frame.height == ui.submit.frame.height && ui.bottom.bezelStyle == ui.submit.bezelStyle, "Footer actions share one native button style")
    let actionFrames = [ui.bottom.frame, ui.submit.frame]
    func commandHeld(_ held: Bool) {
        let event = NSEvent.keyEvent(with: .flagsChanged, location: .zero, modifierFlags: held ? .command : [], timestamp: 0, windowNumber: ui.window.windowNumber, context: nil, characters: "", charactersIgnoringModifiers: "", isARepeat: false, keyCode: 55)!
        ui.window.sendEvent(event)
    }
    commandHeld(true)
    precondition(ui.submit.title == "Create  ⌘↵" && ui.bottom.title == "Add to bottom  ⌘⇧B", "Holding Command reveals each shortcut on its button")
    precondition([ui.bottom.frame, ui.submit.frame] == actionFrames, "Shortcut disclosure must not shift actions")
    commandHeld(false)
    precondition(ui.submit.title == "Create" && ui.bottom.title == "Add to bottom", "Releasing Command hides shortcuts")
    ui.bottom.performClick(nil)
    precondition(ui.bottom.state == .on && ui.bottom.title == "✓ Add to bottom", "The queue toggle visibly confirms its selected state")
    let toggle = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [.command, .shift], timestamp: 0, windowNumber: ui.window.windowNumber, context: nil, characters: "B", charactersIgnoringModifiers: "b", isARepeat: false, keyCode: 11)!
    ui.window.sendEvent(toggle)
    precondition(ui.bottom.state == .off, "The displayed queue shortcut toggles the same control")
    commandHeld(true); ui.windowDidResignKey(Notification(name: NSWindow.didResignKeyNotification))
    precondition(!ui.bottom.showsShortcut && !ui.submit.showsShortcut, "Losing focus clears held-key hints")
    let compactHeight = ui.window.frame.height
    let originalTop = ui.window.frame.maxY
    ui.window.makeFirstResponder(ui.input)
    ui.input.setSelectedRange(NSRange(location: 5, length: 0))
    precondition(!ui.textView(ui.input, doCommandBy: #selector(NSResponder.insertNewline(_:))), "Enter is native newline editing")
    ui.input.insertNewline(nil)
    precondition(ui.input.string == "Title\n" && requests.count == 1, "Return must add a newline without creating an issue")
    precondition(ui.window.frame.height > compactHeight && ui.window.frame.maxY == originalTop, "New lines expand downward, including the trailing blank line")
    ui.input.insertText("Details 🦀\n\n  - Keep indentation", replacementRange: ui.input.selectedRange())
    ui.create()
    let multilineRequest = requests.last!
    precondition(multilineRequest[multilineRequest.firstIndex(of: "--title")! + 1] == "Title")
    precondition(multilineRequest[multilineRequest.firstIndex(of: "--body")! + 1] == "Details 🦀\n\n  - Keep indentation")
    reply?(.failure(StorageError(description: "Disconnected")))
    ui.create(); precondition(requests.last == multilineRequest, "Multiline retries preserve text and request identity")
    reply?(.failure(StorageError(description: "Disconnected")))
    let draft = ui.input.string
    ui.dismiss(); ui.open(cli: nil)
    precondition(ui.input.string == draft, "Dismissal preserves the entire multiline draft")
    ui.current = projects[1]
    ui.input.string = String(repeating: "Wrapped content 🦀 ", count: 150); ui.changed()
    precondition(ui.inputScroll.frame.height <= 240 && ui.input.frame.height > ui.inputScroll.frame.height, "Large drafts scroll within a bounded editor")
    ui.input.setSelectedRange(NSRange(location: ui.input.string.utf16.count, length: 0))
    ui.input.scrollRangeToVisible(ui.input.selectedRange())
    precondition(ui.inputScroll.contentView.bounds.minY > 0, "The end of a long draft remains reachable")
    ui.input.string = "Short"; ui.changed()
    precondition(ui.window.frame.height == compactHeight && ui.inputScroll.contentView.bounds.minY == 0, "Deleting content restores the compact layout: \(ui.window.frame.height) / \(compactHeight), scroll \(ui.inputScroll.contentView.bounds)")
    ui.input.string = "Heading\nDetails @po"; ui.input.setSelectedRange(NSRange(location: ui.input.string.utf16.count, length: 0)); ui.changed()
    precondition(ui.matches == [projects[1]], "Project completion works on subsequent lines")
    precondition(ui.textView(ui.input, doCommandBy: #selector(NSResponder.insertNewline(_:))))
    precondition(ui.input.string == "Heading\nDetails @poe-code ", "Selecting a project preserves line breaks")
    requests = [requests[0]]; ui.current = nil
    ui.input.string = longTitle + " @poe-code"; ui.changed()
    precondition(ui.submit.isEnabled && !ui.overflowNote.isHidden && ui.error.stringValue.isEmpty)
    ui.create()
    let longRequest = requests.last!
    precondition(longRequest[longRequest.firstIndex(of: "--title")! + 1] == longTitle, "Do not truncate text before sending it to the CLI")
    reply?(.failure(StorageError(description: "Disconnected")))
    ui.create(); precondition(requests.last == longRequest, "Long-title retry must preserve request identity")
    reply?(.failure(StorageError(description: "Disconnected")))
    requests.removeLast(2)
    ui.input.string = "Fix @poe-code reconnect"; ui.changed()
    precondition(ui.overflowNote.isHidden, "Shortening the title hides the note")
    precondition(ui.window.makeFirstResponder(ui.input))
    let editor = ui.input
    editor.setSelectedRange(NSRange(location: 13, length: 0)); ui.updatePicker()
    precondition(ui.matches == [projects[1]], "Test Command+Enter while project suggestions and the field editor are active")
    let repeatedReturn = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 0, windowNumber: ui.window.windowNumber, context: nil, characters: "\r", charactersIgnoringModifiers: "\r", isARepeat: true, keyCode: 36)!
    precondition(ui.window.performKeyEquivalent(with: repeatedReturn) && requests.count == 1, "Held Command+Enter must not submit")
    let commandReturn = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 0, windowNumber: ui.window.windowNumber, context: nil, characters: "\r", charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36)!
    editor.setMarkedText("語", selectedRange: NSRange(location: 1, length: 0), replacementRange: NSRange(location: NSNotFound, length: 0))
    precondition(editor.hasMarkedText())
    ui.window.sendEvent(commandReturn)
    precondition(requests.count == 1, "Command+Enter must not submit during input-method composition")
    editor.unmarkText(); ui.input.string = "Fix @poe-code reconnect"
    editor.setSelectedRange(NSRange(location: 13, length: 0)); ui.changed()
    ui.window.sendEvent(commandReturn)
    precondition(ui.saving && !ui.input.isEditable && !ui.closeButton.isEnabled && !ui.bottom.isEnabled)
    precondition(ui.submit.title == "Creating…" && !ui.progress.isHidden)
    precondition(ui.window.contentView is Surface && !ui.window.isOpaque && ui.window.backgroundColor == .clear, "Use the clipped Liquid Glass surface")
    let firstRequest = requests.last!; precondition(firstRequest.contains("--at-top") && firstRequest.contains("human:boss") && firstRequest.contains(projects[1].name))
    ui.create(); precondition(requests.count == 2, "Duplicate Enter while saving")
    precondition(ui.window.performKeyEquivalent(with: commandReturn) && requests.count == 2, "Duplicate Command+Enter while saving")
    ui.dismiss(); precondition(ui.saving, "Cannot close an in-flight mutation")
    reply?(.failure(StorageError(description: "Disconnected")))
    precondition(!ui.saving && ui.input.string == "Fix @poe-code reconnect" && ui.error.stringValue == "Disconnected")
    precondition(ui.submit.title == "Create" && ui.context.stringValue == "Your text is kept", "Failure restores editing and reassures that text is retained")
    let keypadReturn = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 0, windowNumber: ui.window.windowNumber, context: nil, characters: "\u{3}", charactersIgnoringModifiers: "\u{3}", isARepeat: false, keyCode: 76)!
    precondition(ui.window.performKeyEquivalent(with: keypadReturn), "Command+keypad Enter submits too")
    precondition(requests.last == firstRequest, "Retry reuses the mutation ID")
    reply?(.success(Data("{\"issue\":{\"number\":7}}".utf8)))
    precondition(ui.input.string.isEmpty && ui.pending == nil && ui.current == projects[1] && ui.bottom.state == .off)
    precondition(firstRequest[firstRequest.firstIndex(of: "--project")! + 1] == projects[1].name)
    precondition(prefs.string(forKey: "quickIssueProject") == projects[1].name && ui.context.stringValue.contains("#7"))
    precondition(ui.succeeded && ui.input.isHidden && !ui.confirmation.isHidden && ui.submit.isHidden && ui.bottom.isHidden)
    precondition(ui.window.performKeyEquivalent(with: commandReturn) && requests.count == 3, "Success cannot resubmit")
    ui.input.string = "Preserved draft"; ui.bottom.state = .on; ui.dismiss(); ui.open(cli: nil)
    precondition(ui.input.string == "Preserved draft" && ui.bottom.state == .on && ui.current == projects[1])
    ui.create(); precondition(!requests.last!.contains("--at-top"))
    reply?(.failure(StorageError(description: "Retry later")))
    let previous = requests.last!; ui.input.string = "Edited draft"; ui.changed(); ui.create()
    precondition(requests.last!.last != previous.last, "Changed draft gets a fresh request ID")
    reply?(.success(Data("{\"ok\":true}".utf8)))
    precondition(ui.error.stringValue.contains("confirm") && ui.input.string == "Edited draft", "Malformed response preserves draft and request ID")
    ui.input.string = ""; ui.error.stringValue = ""; ui.changed()
    // Render real AppKit controls, not a mock. Screenshots are opt-in and the
    // caller owns cleanup; normal audits leave no visual artifacts behind.
    let renders = ProcessInfo.processInfo.environment["HEY_BOSS_QUICK_ISSUE_RENDERS"]
    if let output = renders ?? ProcessInfo.processInfo.environment["HEY_BOSS_QUICK_ISSUE_SCREENSHOTS"] {
        try! FileManager.default.createDirectory(atPath: output, withIntermediateDirectories: true)
        // Other running apps may take focus during capture; keep this synthetic
        // audit window visible rather than applying production blur dismissal.
        ui.window.delegate = nil
        if renders != nil {
            // Render our own native controls without reading the desktop. Glass
            // needs the window server, so use a plain native surface for this audit.
            ui.canvas.removeFromSuperview(); ui.window.contentView = ui.canvas
            ui.canvas.wantsLayer = true
        }
        ui.window.makeKeyAndOrderFront(nil)
        for dark in [false, true] {
            ui.window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
            ui.window.appearance?.performAsCurrentDrawingAppearance {
                ui.canvas.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
            }
            for state in ["empty", "empty-command", "command", "selected-command", "loading", "projects", "error", "long", "images", "image-error", "saving", "saving-command", "success", "compact", "compact-command", "multiline", "multiline-projects", "multiline-images", "multiline-compact", "scrolling"] {
                ui.images = []; ui.rebuildImages()
                ui.window.setContentSize(NSSize(width: state.contains("compact") ? 520 : 680, height: ui.window.frame.height))
                ui.error.stringValue = ""; ui.overflowNote.stringValue = ""; ui.mention = nil; ui.matches = []; ui.current = projects[1]; ui.showDestination(projects[1])
                ui.showShortcuts(state.contains("command")); ui.bottom.state = state == "selected-command" ? .on : .off
                ui.saving = state.hasPrefix("saving"); ui.succeeded = state == "success"; ui.ready = state != "loading"
                ui.input.string = state.hasPrefix("empty") ? "" : "Fix reconnect @po"
                if state == "loading" { ui.input.string = ""; ui.context.stringValue = "Loading projects…" }
                if state.hasPrefix("saving") { ui.input.string = "Fix reconnect"; ui.context.stringValue = "Creating in poe-code…" }
                if state == "success" { ui.input.string = ""; ui.context.stringValue = "Created #7 in poe-code" }
                if state == "projects" { ui.mention = QuickIssueText.mentions(ui.input.string).first; ui.matches = projects; ui.table.reloadData(); ui.table.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false) }
                if state == "error" { ui.error.stringValue = "Unknown project @missing. Use a known name or full project ID."; ui.input.string = "Fix reconnect @missing" }
                if state == "long" { ui.input.string = String(repeating: "Very long issue title ", count: 30); ui.changed() }
                if ["images", "image-error", "saving", "compact"].contains(state) {
                    let image = NSImage(size: NSSize(width: 240, height: 120), flipped: false) { rect in
                        NSColor.systemIndigo.setFill(); rect.fill()
                        NSColor.systemTeal.setFill(); NSBezierPath(roundedRect: rect.insetBy(dx: 30, dy: 25), xRadius: 12, yRadius: 12).fill()
                        return true
                    }
                    ui.images = (1...5).map { QuickIssueImage(name: "Screenshot \($0).png", data: Data(), thumbnail: image) }; ui.rebuildImages()
                    ui.input.string = "Fix the screenshot preview @poe-code"
                }
                if state.hasPrefix("multiline") {
                    ui.input.string = "Fix reconnect after sleep\n\nSteps to reproduce:\n  - Sleep the Mac\n  - Wake and reconnect 🦀"
                    if state == "multiline-images" {
                        let image = NSImage(size: NSSize(width: 240, height: 120), flipped: false) { rect in NSColor.systemTeal.setFill(); rect.fill(); return true }
                        ui.images = [QuickIssueImage(name: "Reconnect.png", data: Data(), thumbnail: image)]; ui.rebuildImages()
                    }
                    if state == "multiline-projects" { ui.input.string += "\n@po"; ui.mention = QuickIssueText.mentions(ui.input.string).last; ui.matches = projects; ui.table.reloadData() }
                }
                if state == "scrolling" { ui.input.string = "Long draft\n" + String(repeating: "Paragraph with preserved indentation.\n", count: 40) }
                if state == "image-error" { ui.fail("Project is unavailable. Nothing was created. You can edit and try again.") }

                ui.updateEnabled(); ui.layout(); ui.window.contentView!.layoutSubtreeIfNeeded()
                if !ui.succeeded { precondition(ui.context.frame.maxX <= ui.bottom.frame.minX && ui.bottom.frame.maxX < ui.submit.frame.minX, "Destination and buttons stay separated even at compact widths") }
                if !ui.submit.isEnabled { precondition(ui.submit.bezelColor == nil, "Disabled Create uses a neutral native surface") }
                if !ui.succeeded { precondition(ui.inputScroll.frame.maxY <= ui.canvas.bounds.height && ui.inputScroll.frame.minY > ui.mediaHint.frame.maxY, "Editor and hint never overlap") }
                if !ui.picker.isHidden { precondition(ui.inputScroll.frame.minY > ui.heading.frame.maxY && ui.picker.frame.minY > ui.hint.frame.maxY) }
                precondition(ui.window.frame.height <= (ui.window.screen?.visibleFrame.height ?? 900) - 24, "Expanded content stays within the display")
                for row in ui.matches.indices { _ = ui.table.view(atColumn: 0, row: row, makeIfNecessary: true) }
                ui.window.displayIfNeeded()
                RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.05))
                let path = URL(fileURLWithPath: output).appendingPathComponent("\(dark ? "dark" : "light")-\(state).png")
                // Capture the actual window: cacheDisplay omits the controls
                // hosted inside NSGlassEffectView and disrupts its backing view.
                if renders != nil {
                    let bitmap = ui.canvas.bitmapImageRepForCachingDisplay(in: ui.canvas.bounds)!
                    ui.canvas.cacheDisplay(in: ui.canvas.bounds, to: bitmap)
                    try! bitmap.representation(using: .png, properties: [:])!.write(to: path)
                } else {
                    let capture = Process(); capture.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
                    capture.arguments = ["-x", "-o", "-l", String(ui.window.windowNumber), path.path]
                    try! capture.run(); capture.waitUntilExit()
                    precondition(capture.terminationStatus == 0, "Window screenshots require Screen Recording access")
                }
                let bitmap = NSBitmapImageRep(data: try! Data(contentsOf: path))!
                for (x, y) in renders != nil ? [] : [(0, 0), (bitmap.pixelsWide - 1, 0), (0, bitmap.pixelsHigh - 1), (bitmap.pixelsWide - 1, bitmap.pixelsHigh - 1)] {
                    precondition(bitmap.colorAt(x: x, y: y)!.alphaComponent == 0, "Panel corners must remain transparent")
                }
            }
        }
    }
    ui.window.close()
    print("Passed: native quick-add parsing, Unicode, project matching, Boss identity, queue placement, in-flight guard, draft retention and idempotent retry")
}

final class QuickIssuePasteAuditText: NSTextView {
    var pasteCalls = 0
    override func paste(_ sender: Any?) { pasteCalls += 1 }
}

func auditQuickIssueMedia() {
    let suite = "hey-boss-media-\(UUID().uuidString)"
    let prefs = UserDefaults(suiteName: suite)!
    defer { prefs.removePersistentDomain(forName: suite) }
    let ui = NativeQuickIssue(present: false, preferences: prefs)
    ui.ready = true; ui.current = QuickIssueProject(id: "named:Media", name: "Media")
    ui.projects = [ui.current!]; ui.input.string = "Fix screenshot"; ui.changed()
    let mediaButtons = ui.canvas.subviews.compactMap { $0 as? NSButton }
    precondition(!mediaButtons.contains { ["Dictate", "Paste image"].contains($0.title) }, "Quick Add keeps screenshot paste on the keyboard without media buttons")
    precondition(ui.mediaHint.frame.minX == ui.context.frame.minX, "The paste hint aligns with the footer destination")
    precondition(ui.window.frame.height == 166, "The keyboard hint uses a compact row")
    let image = NSImage(size: NSSize(width: 240, height: 120), flipped: false) { rect in
        NSColor.systemBlue.setFill(); rect.fill(); return true
    }
    let board = NSPasteboard.withUniqueName()
    defer { board.releaseGlobally() }
    board.writeObjects([image])
    // Exercise the actual shortcut without touching the user's clipboard.
    ui.window.pasteImage = { ui.pasteImages(board) }
    let paste = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 0, windowNumber: ui.window.windowNumber, context: nil, characters: "v", charactersIgnoringModifiers: "v", isARepeat: false, keyCode: 9)!
    ui.window.makeFirstResponder(ui.bottom)
    precondition(ui.window.performKeyEquivalent(with: paste) && ui.images.count == 1, "Command-V attaches with button focus too")
    precondition(ui.input.string == "Fix screenshot", "Image paste preserves the title")
    ui.window.makeFirstResponder(ui.input)
    ui.window.sendEvent(paste)
    precondition(ui.images.count == 2, "Command-V reaches images through the field editor")
    let repeatPaste = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 0, windowNumber: ui.window.windowNumber, context: nil, characters: "v", charactersIgnoringModifiers: "v", isARepeat: true, keyCode: 9)!
    ui.window.sendEvent(repeatPaste)
    precondition(ui.images.count == 2, "Holding Command-V must not duplicate attachments")
    let fallback = QuickIssuePasteAuditText(frame: .zero)
    ui.canvas.addSubview(fallback); ui.window.makeFirstResponder(fallback)
    board.clearContents(); board.setString("ordinary text", forType: .string)
    ui.window.sendEvent(paste)
    precondition(fallback.pasteCalls == 1, "Text-only paste reaches the native editor")
    ui.window.makeFirstResponder(ui.input); fallback.removeFromSuperview()
    board.clearContents(); board.writeObjects([image])
    ui.images = []; ui.rebuildImages()
    precondition(ui.pasteImages(board) && ui.images.count == 1)
    precondition(ui.images[0].name.hasSuffix(".png"), "Clipboard TIFF is normalized for web preview")
    ui.dismiss(); precondition(ui.images.count == 1, "Dismissal retains pasted images")
    var calls: [[String]] = []; var reply: ((Result<Data, Error>) -> Void)?
    ui.runner = { args, completion in calls.append(args); reply = completion }
    ui.create(); precondition(!calls.last!.contains("--draft"), "Image tasks must never be silently converted to drafts")
    precondition(calls.last!.filter { $0 == "--attachment" }.count == 1)
    let rejected = calls.last!
    let rejectedFile = rejected[rejected.firstIndex(of: "--attachment")! + 1]
    reply?(.failure(QuickIssueRequestError(message: "Project is unavailable.", rejected: true)))
    precondition(ui.input.isEditable && ui.bottom.isEnabled, "Rejected saves restore editing")
    precondition(ui.mediaSubmission == nil && ui.images.count == 1 && ui.input.string == "Fix screenshot")
    precondition(ui.error.stringValue.contains("Nothing was created") && !ui.context.stringValue.contains("Draft"))
    precondition(!FileManager.default.fileExists(atPath: rejectedFile))
    ui.input.string = "Fixed screenshot"; ui.changed(); ui.create()
    let save = calls.last!
    let file = save[save.firstIndex(of: "--attachment")! + 1]
    ui.window.sendEvent(paste)
    precondition(ui.images.count == 1, "Cannot add images during submission")
    reply?(.failure(StorageError(description: "Response lost")))
    precondition(!ui.saving && !ui.input.isEditable && ui.images.count == 1 && ui.submit.title == "Retry")
    precondition(!ui.pasteImages(board), "An uncertain submission stays immutable")
    ui.dismiss(); precondition(ui.images.count == 1 && ui.mediaSubmission != nil, "Closing preserves retry state")
    ui.create(); precondition(calls.last! == save, "Retry saves the exact same task and images without duplication")
    reply?(.success(Data("{\"issue\":{\"number\":42}}".utf8)))
    precondition(ui.succeeded && ui.images.isEmpty && ui.mediaSubmission == nil)
    precondition(!FileManager.default.fileExists(atPath: file), "Upload staging files are removed")
    precondition(calls.allSatisfy { $0.first != "undraft" && $0.first != "attachment" }, "No draft or partial upload steps")

    ui.succeeded = false; ui.input.string = "Fix screenshot"; ui.changed()

    board.clearContents(); board.setString("ordinary text", forType: .string)
    precondition(!ui.pasteImages(board), "Plain text keeps normal paste behavior")
    board.clearContents(); board.setData(Data("broken".utf8), forType: .png)
    precondition(ui.pasteImages(board) && ui.images.isEmpty && !ui.error.stringValue.isEmpty)
    board.clearContents(); board.setData(Data(repeating: 0, count: 10 * 1024 * 1024 + 1), forType: .png)
    precondition(ui.pasteImages(board) && ui.images.isEmpty && ui.error.stringValue.contains("10 MiB"))
    let files = FileManager.default.temporaryDirectory.appendingPathComponent("hb-clipboard-files-" + UUID().uuidString)
    try! FileManager.default.createDirectory(at: files, withIntermediateDirectories: false)
    defer { try? FileManager.default.removeItem(at: files) }
    let png = try! ArtifactImageInput(url: nil, bytes: image.tiffRepresentation!, name: "Screenshot.tiff").contents()
    let first = files.appendingPathComponent("Screen one.png"), second = files.appendingPathComponent("Screen two.png")
    try! png.data.write(to: first); try! png.data.write(to: second)
    board.clearContents(); board.writeObjects([first as NSURL, second as NSURL])
    precondition(ui.pasteImages(board) && ui.images.map(\.name) == ["Screen one.png", "Screen two.png"], "Copied Finder images retain filenames and order")
    precondition(ui.images.allSatisfy { $0.data == png.data }, "Image contents survive paste")
    ui.images = []; ui.rebuildImages()
    board.clearContents(); board.writeObjects([image])
    for _ in 0..<8 { precondition(ui.pasteImages(board)) }
    precondition(ui.images.count == 8)
    precondition(ui.pasteImages(board) && ui.images.count == 8 && !ui.error.stringValue.isEmpty)
    let remove = ui.imageCanvas.subviews.compactMap { $0 as? NSButton }.first!
    ui.removeImage(remove); precondition(ui.images.count == 7)
    board.clearContents(); board.writeObjects([first as NSURL, second as NSURL])
    precondition(ui.pasteImages(board) && ui.images.count == 7, "An over-limit batch is rejected without partially adding files")
    while !ui.images.isEmpty {
        ui.removeImage(ui.imageCanvas.subviews.compactMap { $0 as? NSButton }.first!)
    }
    precondition(ui.imageStrip.isHidden && ui.window.frame.height == 166, "Removing all images collapses the preview strip")
    ui.window.pasteImage = nil
    ui.window.close()
    print("Passed: native pasted images, atomic creation, editable rejection, safe retries, cleanup and keyboard-only media controls")

}

func auditQuickIssueMediaIntegration() {
    precondition(ProcessInfo.processInfo.environment["HEY_BOSS_ISSUE_DB"]?.hasPrefix("/tmp/") == true, "Use an isolated temporary issue store")
    let initialize = Process()
    initialize.executableURL = URL(fileURLWithPath: ProcessInfo.processInfo.environment["HEY_BOSS_CLI_PATH"]!)
    initialize.arguments = ["project", "init", "--project", "Native media test", "--prs", "false", "--worktree", "false", "--drafts", "false", "--yes", "--json"]
    initialize.currentDirectoryURL = URL(fileURLWithPath: ProcessInfo.processInfo.environment["HEY_BOSS_ISSUE_DB"]!).deletingLastPathComponent()
    initialize.standardOutput = FileHandle.nullDevice
    try! initialize.run(); initialize.waitUntilExit()
    precondition(initialize.terminationStatus == 0, "Initialize only the isolated test project")
    let suite = "hey-boss-quick-integration"; let prefs = UserDefaults(suiteName: suite)!
    defer { prefs.removePersistentDomain(forName: suite) }
    let ui = NativeQuickIssue(present: false, preferences: prefs)
    ui.ready = true; ui.current = QuickIssueProject(id: "named:Native media test", name: "Native media test")
    ui.projects = [ui.current!]; ui.input.string = "Screenshot regression\n\nSteps:\n  - Paste two images\n  - Preserve paragraphs 🦀\n\n    code"; ui.changed()
    let board = NSPasteboard.withUniqueName(); defer { board.releaseGlobally() }
    let sample = NSImage(size: NSSize(width: 120, height: 80), flipped: false) { rect in NSColor.systemTeal.setFill(); rect.fill(); return true }
    board.writeObjects([sample]); precondition(ui.pasteImages(board)); precondition(ui.pasteImages(board))
    func wait(_ condition: () -> Bool) {
        let deadline = Date(timeIntervalSinceNow: 45)
        while !condition() && Date() < deadline { RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.02)) }
        precondition(condition(), "Native CLI operation timed out")
    }
    var configured: Result<Data, Error>?
    ui.request(["settings", "set", "--project", "Native media test", "--no-drafts", "--json"]) { configured = $0 }
    wait { configured != nil }
    _ = try! configured!.get()
    ui.create()
    wait { !ui.saving }
    precondition(ui.succeeded, ui.error.stringValue)
    var result: Result<Data, Error>?
    ui.request(["view", "1", "--project", "Native media test", "--json"]) { result = $0 }; wait { result != nil }
    let value = try! JSONSerialization.jsonObject(with: result!.get()) as! [String: Any]
    let issue = value["issue"] as! [String: Any]
    precondition(issue["draft"] as? Bool == false && issue["title"] as? String == "Screenshot regression")
    precondition(issue["body"] as? String == "Steps:\n  - Paste two images\n  - Preserve paragraphs 🦀\n\n    code", "The real issue store preserves multiline details")
    result = nil
    ui.request(["attachment", "list", "--issue", "1", "--project", "Native media test", "--json"]) { result = $0 }; wait { result != nil }
    let listed = try! JSONSerialization.jsonObject(with: result!.get()) as! [String: Any]
    let attachments = listed["attachments"] as! [[String: Any]]
    precondition(attachments.count == 2 && attachments.allSatisfy { ($0["name"] as? String)?.hasSuffix(".png") == true })
    ui.window.close()
    print("Passed: real CLI atomically saves a regular task and two images with drafts disabled")
}

func auditInbox(root: URL, sample: Record) {
    let overview = AgentsOverview(present: false, cli: nil)
    var openedInbox = 0, openedIssues = 0
    overview.openInbox = { openedInbox += 1 }
    overview.openIssues = { openedIssues += 1 }
    precondition(overview.inboxMenuItem.menu != nil && overview.issuesMenuItem.menu === overview.inboxMenuItem.menu)
    for item in [overview.inboxMenuItem, overview.issuesMenuItem] {
        precondition(overview.validateMenuItem(item))
        precondition(NSApp.sendAction(item.action!, to: item.target, from: item))
    }
    precondition(openedInbox == 1 && openedIssues == 1)
    let launcher = IssuesLauncher()
    var launches = 0, urls: [URL] = [], failures: [String] = []
    launcher.openURL = { urls.append($0); return true }
    launcher.report = { failures.append($0) }
    launcher.start = { _ in launches += 1 }
    launcher.probe = { $0(true) }
    launcher.open(cli: nil)
    precondition(launches == 0 && urls.count == 1 && failures.isEmpty, "Reuse the running issue service")
    launcher.open(cli:nil,page:.inbox)
    precondition(urls.last?.fragment == "view=inbox", "Inbox opens the shared web interface")
    var probes = 0
    launcher.probe = { probes += 1; $0(probes > 1) }
    launcher.open(cli: "/usr/bin/true")
    precondition(launches == 1 && urls.count == 3 && !launcher.launching)
    precondition(urls.last?.path == "/" && urls.last?.fragment == "view=issues", "Issues resets the destination to the normal list")
    launcher.probe = { $0(false) }
    launcher.open(cli: nil)
    precondition(failures.count == 1 && !launcher.launching)
    var ready: ((Bool) -> Void)?
    launcher.probe = { ready = $0 }
    launcher.open(cli: nil); launcher.open(cli: nil, page: .inbox)
    ready?(true)
    precondition(urls.count == 4 && urls.last?.fragment == "view=inbox" && !launcher.launching, "Repeated menu clicks must open the latest destination without starting duplicate servers")
    launcher.probe = { $0(true) }
    launcher.open(cli: nil, page: .quickIssue)
    precondition(urls.last?.fragment == "quick-issue=1" && launches == 1, "Quick add opens above the shared interface without starting another service")
    let store = try! Store(root.appendingPathComponent("inbox-dismiss.db").path)
    let ui = Interface(present: false)
    store.removeMany = { ui.remove($0) }
    ui.onDismissMany = { store.dismiss($0) }
    for n in 0..<3 {
        var row = Record(taskID: "review-close-\(n)", kind: "update", question: "Review", project: nil, title: "Review \(n)", description: "", options: [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: Double(n), presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
        row.commentsEnabled = true
        try! store.database.save(row); ui.add(row)
    }
    ui.hideStack.performClick(nil)
    precondition(ui.stackHiddenByUser && ui.cards.count == 3)
    ui.layout()
    precondition(ui.stackHiddenByUser)
    ui.projectGroups["Notifications"]!.clear.performClick(nil)
    precondition(ui.cards.isEmpty && (try! store.database.pending()).isEmpty, "Project dismissal must work for unnamed groups and open document reviews")
    let first = Record(taskID: "first-question", kind: "prompt", question: "First?", project: "Inbox", title: "First", description: "", options: [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: 1, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
    let second = Record(taskID: "second-question", kind: "prompt", question: "Second?", project: "Inbox", title: "Second", description: "", options: [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: 2, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
    ui.add(first); ui.add(second)
    ui.field!.stringValue = "Keep my unfinished answer"
    ui.presentQuestion(second)
    precondition(ui.current?.taskID == second.taskID)
    ui.presentQuestion(first)
    precondition(ui.field?.stringValue == "Keep my unfinished answer" && ui.questions.count == 1)
    print("Passed: web Inbox/Issues menu actions, service reuse/start/errors, grouped review dismissal, hidden stack, question draft preservation")
}

func auditDismissalAnimation(sample: Record) {
    let ui = Interface(present: false)
    let fixtures = (0..<4).map { index -> Record in
        Record(taskID: "animation-\(index)", kind: "update", question: sample.question,
               project: sample.project, title: sample.title, description: sample.description,
               options: [], autoclose: nil, linkURL: nil, linkLabel: nil,
               createdAt: Date().timeIntervalSince1970 + Double(index), presentedAt: nil,
               expiresAt: nil, status: "pending", result: nil, origin: nil)
    }
    for item in fixtures.prefix(3) { ui.add(item) }
    ui.expandedProjects.insert(sample.project!)
    ui.layout()
    let departing = ui.cards.removeFirst()
    let originalParent = departing.view.superview
    ui.finishCardRemoval([departing], animated: true)
    precondition(ui.dismissalAnimations == 1 && departing.view.superview === originalParent)
    // New arrivals must not reparent a grouped card midway through its fade.
    ui.add(fixtures[3])
    precondition(ui.cards.last!.view.isHidden, "Arrival must not flash at an unlaid-out origin")
    precondition(departing.view.superview === originalParent)
    let second = ui.cards.removeFirst()
    ui.finishCardRemoval([second], animated: true)
    precondition(ui.dismissalAnimations == 2)
    let completionDeadline = Date().addingTimeInterval(3)
    while ui.dismissalAnimations > 0 && Date() < completionDeadline {
        RunLoop.main.run(until: Date().addingTimeInterval(0.02))
    }
    print("Animation completion: active=\(ui.dismissalAnimations), departing=\(departing.view.superview != nil), second=\(second.view.superview != nil), cards=\(ui.cards.count), groups=\(ui.projectGroups.count), survivors=\(ui.cards.map { ($0.view.superview === ui.document, $0.view.alphaValue, $0.view.isHidden) })")
    precondition(ui.dismissalAnimations == 0, "Dismissal animations must finish")
    precondition(departing.view.superview == nil && second.view.superview == nil, "Departing cards must detach")
    precondition(ui.cards.count == 2 && ui.projectGroups.isEmpty)
    precondition(ui.cards.allSatisfy { $0.view.superview === ui.document && $0.view.alphaValue == 1 && !$0.view.isHidden }, "Survivors must be visible and correctly parented")
    ui.remove(ui.cards.map { $0.row.taskID })
    precondition(ui.cards.isEmpty && ui.dismissalAnimations == 0)
    print("Passed: overlapping dismissal fades retain grouped views, arrivals wait for fade, grouping threshold settles, survivors stay opaque, immediate nonanimated removal")
}

func auditScannerOutput(root: URL) {
    let path = root.appendingPathComponent("scanner-output")
    precondition(FileManager.default.createFile(atPath: path.path, contents: nil))
    let writer = try! FileHandle(forWritingTo: path)
    try! writer.truncate(atOffset: 8 * 1024 * 1024)
    let reader = try! FileHandle(forReadingFrom: path)
    precondition(try! readScannerOutput(reader).count == 8 * 1024 * 1024)
    try! writer.truncate(atOffset: 8 * 1024 * 1024 + 1)
    try! reader.seek(toOffset: 0)
    do { _ = try readScannerOutput(reader); preconditionFailure("Oversized scanner output accepted") }
    catch { precondition(error is StorageError) }
    try! reader.close(); try! writer.close()
    print("Passed: scanner output accepts its byte boundary and rejects oversized results")
}

func auditSocketDeadlines() {
    for payload in [Data(), Data("not JSON".utf8)] {
        var peers = [Int32](repeating: 0, count: 2)
        precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &peers) == 0)
        if !payload.isEmpty { _ = payload.withUnsafeBytes { Darwin.write(peers[1], $0.baseAddress, $0.count) } }
        Darwin.close(peers[1])
        let data = try! readRequest(peers[0])
        precondition((try? JSONDecoder().decode(Request.self, from: data)) == nil)
        Darwin.close(peers[0])
    }
    var stalled = [Int32](repeating: 0, count: 2)
    var unread = [Int32](repeating: 0, count: 2)
    precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &stalled) == 0)
    precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &unread) == 0)
    var sendBuffer: Int32 = 4096
    precondition(setsockopt(unread[1], SOL_SOCKET, SO_SNDBUF, &sendBuffer, socklen_t(MemoryLayout<Int32>.size)) == 0)
    defer { for fd in stalled { Darwin.close(fd) }; Darwin.close(unread[0]) }
    let reply = Reply(unread[1])
    reply.send(["result":String(repeating: "x", count: 1024 * 1024)])
    let start = DispatchTime.now().uptimeNanoseconds
    do { _ = try readRequest(stalled[0]); preconditionFailure("Stalled request was accepted") }
    catch { precondition(error is StorageError) }
    let seconds = Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000_000
    fputs("Socket deadline audit: stalled read \(seconds)s\n", stderr)
    precondition(seconds >= 9 && seconds < 13)
    // The read and write deadlines coincide. Keep the peer unread until the
    // asynchronous write-cancellation callback has had time to run.
    Thread.sleep(forTimeInterval: 1)
    var bytes = [UInt8](repeating: 0, count: 8192)
    var total = 0
    while true {
        var descriptor = pollfd(fd: unread[0], events: Int16(POLLIN), revents: 0)
        let ready = poll(&descriptor, 1, 2000)
        if ready <= 0 { fputs("Socket deadline audit: response still open after \(total) bytes\n", stderr) }
        precondition(ready > 0, "Stalled response did not close")
        let count = Darwin.read(unread[0], &bytes, bytes.count)
        if count == 0 { break }
        precondition(count > 0)
        total += count
    }
    fputs("Socket deadline audit: response closed after \(total) bytes\n", stderr)
    precondition(total > 0 && total < 1024 * 1024)
    withExtendedLifetime(reply) {}
    print("Passed: stalled socket request and unread response close within bounded deadlines")
}

func auditCloseAll(root: URL) {
    let store = try! Store(root.appendingPathComponent("close-all.db").path)
    let ui = Interface(present: false)
    store.show = { ui.add($0) }
    store.remove = { ui.remove($0) }
    store.removeMany = { ui.remove($0) }
    ui.onComplete = { store.complete($0, $1) }
    ui.onCompleteMany = { store.complete($0) }
    ui.onPresented = { store.presented($0, $1) }
    func item(_ kind: String, _ project: String) -> Record {
        Record(taskID: UUID().uuidString, kind: kind, question: "Review item", project: project, title: "Review", description: "Ready to review", options: kind == "approval" ? ["Yes", "No"] : [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: 0, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
    }
    func responseSocket() -> (Reply, FileHandle) {
        var fds: [Int32] = [0, 0]
        precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
        return (Reply(fds[1]), FileHandle(fileDescriptor: fds[0], closeOnDealloc: true))
    }
    func response(_ handle: FileHandle) -> [String: String] {
        try! JSONDecoder().decode([String: String].self, from: handle.readToEnd()!)
    }
    let prompt = item("prompt", "Atlas")
    (try! store.database.save(prompt))
    ui.add(prompt)
    precondition(ui.cards.isEmpty && ui.stackToolbar.window === ui.stack)
    precondition(ui.stackToolbar.frame.maxY <= ui.stack.contentView!.bounds.maxY)
    let approval = item("approval", "Orion")
    let cards = (0..<3).map { _ in item("update", "Atlas") } + [item("alert", "Orion")]
    let visible = cards + [prompt, approval]
    for row in cards + [approval] { (try! store.database.save(row)); ui.add(row) }
    precondition(ui.projectGroups.count == 1 && ui.questions.count == 1)
    ui.openPreview(cards[0])
    let unseen = item("alert", "Unshown")
    var answered = item("approval", "Archive")
    answered.status = "ok"
    answered.result = "Yes"
    for row in [unseen, answered] { (try! store.database.save(row)) }
    let activeWaiter = responseSocket()
    let queuedWaiter = responseSocket()
    store.waiters[prompt.taskID] = [activeWaiter.0]
    store.waiters[approval.taskID] = [queuedWaiter.0]
    var snapshot: [String] = []
    ui.onDismissMany = { snapshot = $0 }
    ui.closeAll.performClick(nil)
    precondition(Set(snapshot) == Set(visible.map(\.taskID)))
    let arriving = item("alert", "New arrival")
    (try! store.database.save(arriving))
    ui.add(arriving)
    store.dismiss(snapshot)
    for row in visible {
        let stored = (try! store.database.get(row.taskID))
        precondition(stored.status == (row.kind == "prompt" || row.kind == "approval" ? "cancelled" : "ok"))
        precondition(stored.result == nil && stored.completedAt != nil)
    }
    for reply in [response(activeWaiter.1), response(queuedWaiter.1)] {
        precondition(reply["status"] == "cancelled" && reply["result"] == nil)
    }
    let laterWait = responseSocket()
    let request = try! JSONDecoder().decode(Request.self, from: JSONSerialization.data(withJSONObject: ["command": "wait", "task_id": prompt.taskID, "sync": false]))
    store.handle(request, laterWait.0)
    precondition(response(laterWait.1)["status"] == "cancelled")
    store.complete(prompt.taskID, "stale answer")
    precondition((try! store.database.get(prompt.taskID)).status == "cancelled")
    precondition(ui.cards.map { $0.row.taskID } == [arriving.taskID])
    precondition(ui.current == nil && ui.questions.isEmpty && ui.question.contentView == nil)
    precondition(ui.previews.count == 1)
    precondition((try! store.database.get(answered.taskID)).result == "Yes")
    var restored: [Record] = []
    store.show = { restored.append($0) }
    store.restore()
    precondition(Set(restored.map(\.taskID)) == Set([unseen.taskID, arriving.taskID]))
    ui.onDismissMany = { store.dismiss($0) }
    ui.closeAll.performClick(nil)
    precondition(ui.cards.isEmpty && !ui.stack.isVisible)
    for preview in ui.previews.values { preview.close() }
    for kind in ["prompt", "approval"] {
        let replayed = item(kind, "Remote hide")
        try! store.database.save(replayed)
        ui.add(replayed)
        let waiter = responseSocket()
        store.waiters[replayed.taskID] = [waiter.0]
        let hidden = responseSocket()
        let hide = try! JSONDecoder().decode(Request.self, from: JSONSerialization.data(withJSONObject: ["command":"hide", "task_id":replayed.taskID, "sync":false]))
        store.handle(hide, hidden.0)
        for result in [response(hidden.1), response(waiter.1)] {
            precondition(result["status"] == "cancelled" && result["result"] == nil)
        }
        precondition(try! store.database.get(replayed.taskID).status == "cancelled")
        precondition(ui.current == nil && ui.questions.isEmpty)
    }
    print("Passed: global Close all, grouped/multi-project dismissal, active/queued cancellation, waiter completion, history/readers preserved, concurrent arrivals retained")
}


func auditAppearance(root: URL, sample: Record) {
    let legacy = try! JSONDecoder().decode(Record.self, from: JSONEncoder().encode(sample))
    precondition(legacy.visualSeverity == .neutral && legacy.iconData == nil)
    for name in iconSymbols.values { precondition(NSImage(systemSymbolName: name, accessibilityDescription: nil) != nil, "Unavailable symbol: \(name)") }
    var problem = sample
    problem.severity = "error"
    problem.icon = "not.a.real.symbol"
    let fallback = IconBadge(problem, frame: NSRect(x: 0, y: 0, width: 32, height: 32))
    precondition(fallback.imageView.image != nil && fallback.accessibilityLabel() == "Error")
    let customFile = root.appendingPathComponent("icon.png")
    let art = NSImage(size: NSSize(width: 160, height: 120), flipped: false) { rect in
        let shape = NSBezierPath(roundedRect: rect, xRadius: 30, yRadius: 30)
        NSGradient(starting: NSColor(srgbRed: 0.46, green: 0.30, blue: 0.95, alpha: 1), ending: NSColor(srgbRed: 0.20, green: 0.14, blue: 0.55, alpha: 1))!.draw(in: shape, angle: -90)
        ("A" as NSString).draw(in: NSRect(x: 47, y: 16, width: 80, height: 86), withAttributes: [.font: NSFont.systemFont(ofSize: 78, weight: .semibold), .foregroundColor: NSColor.white])
        return true
    }
    let bitmap = NSBitmapImageRep(data: art.tiffRepresentation!)!
    try! bitmap.representation(using: .png, properties: [:])!.write(to: customFile)
    problem.iconData = snapshotIcon(customFile.path)
    precondition(problem.iconData != nil)
    let snapshot = NSBitmapImageRep(data: problem.iconData!)!
    precondition(snapshot.pixelsWide == 128 && snapshot.pixelsHigh == 128)
    precondition(snapshot.colorAt(x: 64, y: 0)!.alphaComponent == 0)
    precondition(snapshot.colorAt(x: 64, y: 64)!.alphaComponent > 0)
    precondition(snapshotIcon(root.appendingPathComponent("missing.png").path) == nil)
    let invalid = root.appendingPathComponent("invalid.png")
    try! Data("not an image".utf8).write(to: invalid)
    precondition(snapshotIcon(invalid.path) == nil)
    // Build a valid, highly compressed 4097×4097 grayscale PNG. Its pixel area
    // exceeds the limit while the compressed file is only a few kilobytes.
    let raw = Data(count: (4097 + 1) * 4097)
    var compressed = Data(count: Int(compressBound(uLong(raw.count))))
    var compressedCount = uLongf(compressed.count)
    let compressedResult = raw.withUnsafeBytes { input in
        compressed.withUnsafeMutableBytes { output in
            compress2(output.bindMemory(to: Bytef.self).baseAddress, &compressedCount, input.bindMemory(to: Bytef.self).baseAddress, uLong(raw.count), Z_BEST_COMPRESSION)
        }
    }
    precondition(compressedResult == Z_OK)
    compressed.count = Int(compressedCount)
    func pngChunk(_ type: String, _ payload: Data) -> Data {
        var length = UInt32(payload.count).bigEndian
        var result = withUnsafeBytes(of: &length) { Data($0) }
        let content = Data(type.utf8) + payload
        result.append(content)
        var checksum = UInt32(content.withUnsafeBytes { crc32(0, $0.bindMemory(to: Bytef.self).baseAddress, uInt(content.count)) }).bigEndian
        result.append(withUnsafeBytes(of: &checksum) { Data($0) })
        return result
    }
    var oversized = Data([137, 80, 78, 71, 13, 10, 26, 10])
    oversized.append(pngChunk("IHDR", Data([0, 0, 16, 1, 0, 0, 16, 1, 8, 0, 0, 0, 0])))
    oversized.append(pngChunk("IDAT", compressed))
    oversized.append(pngChunk("IEND", Data()))
    precondition(oversized.count < 4 * 1024 * 1024)
    let source = CGImageSourceCreateWithData(oversized as CFData, [kCGImageSourceShouldCache: false] as CFDictionary)!
    let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil)! as NSDictionary
    precondition((properties[kCGImagePropertyPixelWidth] as! NSNumber).intValue == 4097)
    precondition((properties[kCGImagePropertyPixelHeight] as! NSNumber).intValue == 4097)
    precondition(boundedIconImage(oversized) == nil)
    let oversizedFile = root.appendingPathComponent("oversized-pixels.png")
    try! oversized.write(to: oversizedFile)
    precondition(snapshotIcon(oversizedFile.path) == nil)
    let pdf = NSView(frame: NSRect(x: 0, y: 0, width: 200, height: 100)).dataWithPDF(inside: NSRect(x: 0, y: 0, width: 200, height: 100))
    precondition(boundedIconImage(pdf) != nil)
    let saved = try! Database(root.appendingPathComponent("appearance.db").path)
    (try! saved.save(problem))
    try! FileManager.default.removeItem(at: customFile)
    let restored = (try! saved.get(problem.taskID))
    precondition(restored.visualSeverity == .error && restored.iconData == problem.iconData)
    let inbox = try! saved.inboxRows()
    precondition(inbox.count == 1)
    precondition(inbox[0]["severity"] as? String == "error")
    precondition(inbox[0]["icon"] as? String == problem.icon)
    precondition(inbox[0]["iconData"] as? String == problem.iconData!.base64EncodedString())
    precondition(inbox[0]["question"] == nil && inbox[0]["attachment"] == nil)

    precondition(IconBadge(restored, frame: NSRect(x: 0, y: 0, width: 32, height: 32)).usesCustomImage)
    var regular = sample
    regular.severity = "info"
    let cards = [regular, problem, sample].map { Card($0, open: {}, openURL: { _ in }, complete: { _, _ in }) }
    cards.forEach { $0.configure(grouped: true) }
    let group = ProjectGroup("Atlas", toggle: {}, clear: {})
    group.update(cards, expanded: false)
    precondition(group.summary.stringValue.hasPrefix("Error · "))
    precondition(group.badge!.row.visualSeverity == .error)
    group.update(cards, expanded: true)
    precondition(group.badge!.isHidden)
    let directory = ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_SNAPSHOT_DIR"]
    if let directory {
        try! FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
        for dark in [false, true] { appearanceSnapshot(sample: sample, custom: problem, directory: directory, dark: dark) }
    }
    // Keep geometry coverage in the ordinary audit, even without PNG exports.
    for dark in [false, true] { densitySnapshot(custom: problem, directory: directory, dark: dark) }
    print("Passed: legacy appearance, SF Symbols, safe image fallback, durable custom icon, collapsed error visibility")
}

final class AuditCanvas: NSView {
    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        bounds.fill()
        NSColor.controlBackgroundColor.setFill()
        for view in subviews where view is Surface {
            let card = NSBezierPath(roundedRect: view.frame, xRadius: 14, yRadius: 14)
            NSColor.controlBackgroundColor.setFill()
            card.fill()
            NSColor.separatorColor.setStroke()
            card.lineWidth = 0.5
            card.stroke()
        }
    }
}

func densitySnapshot(custom: Record, directory: String?, dark: Bool) {
    func fixture(_ kind: String, _ title: String, _ message: String, severity: String? = nil, options: [String] = []) -> Record {
        var row = Record(taskID: UUID().uuidString, kind: kind, question: kind == "update" ? "# Report" : message, project: "Atlas", title: title, description: kind == "update" ? message : "", options: options, autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: 0, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
        row.severity = severity
        return row
    }
    let rows = [
        fixture("update", "Review ready", "Three changes are ready for review."),
        fixture("update", "Authentication migration review", "The migration preserves current sessions. Two changes need review before rollout.", severity: "info"),
        fixture("alert", "Dependency warning", "A required package is outdated. Review the compatibility notes before upgrading.", severity: "warning"),
        fixture("update", "Build failed", "Two integration tests failed. Failure logs and a proposed fix are ready.", severity: "error"),
        fixture("alert", "Checks passed", "All 13 tests passed.", severity: "success")
    ]
    var keep: [Card] = []
    let sources: [String?] = ["This Mac", "devbox", nil, "build-server-with-a-long-hostname.staging.example.com", "This Mac"]
    for (index, sample) in rows.enumerated() {
        var row = sample
        row.sourceHost = sources[index]
        row.sourceKnown = sources[index] != nil
        if index == 3 { row.iconData = custom.iconData }
        let card = Card(row, open: {}, openURL: { _ in }, complete: { _, _ in })
        card.configure(grouped: false)
        precondition(card.body.frame.width == 312)
        precondition(card.header.frame.maxX <= card.info.frame.minX)
        if let button = card.link {
            precondition(!card.header.frame.intersects(button.frame))
            precondition(card.body.frame.minY >= button.frame.maxY)
        }
        precondition(!card.header.frame.intersects(card.close.frame))
        precondition(card.body.frame.maxY <= card.header.frame.minY)
        let source = card.view.content.subviews.compactMap { $0 as? NSTextField }.first { $0.stringValue == row.sourceLabel }!
        precondition(!source.isHidden && source.frame.height == 14)
        precondition(source.frame.maxY + 6 <= card.body.frame.minY, "Source attribution must not overlap the summary")
        precondition(source.frame.minY >= 12 && source.frame.maxX <= 328)
        precondition(source.toolTip == (row.isLocalSource || row.sourceHost == nil ? row.sourceLabel : "Sent from server: \(row.sourceLabel)"))
        if let button = card.link {
            precondition(button.frame.maxY + 8 <= source.frame.minY, "Source attribution must clear the action row")
        }
        // A one-line summary, source row and action fit in 125 px. Two-line
        // updates stay within 140 px; alerts without an action stay within 91 px.
        if index == 0 {
            precondition((17...19).contains(card.body.frame.height))
            precondition((123...125).contains(card.view.frame.height), "One-line update density changed")
        }
        if row.kind == "update" { precondition(card.view.frame.height <= 140) }
        if index == 4 { precondition(card.link == nil && card.view.frame.height <= 91) }
        keep.append(card)
    }
    let stackHeight = keep.reduce(CGFloat(0)) { $0 + $1.view.frame.height } + CGFloat(keep.count - 1) * 8
    let canvas = AuditCanvas(frame: NSRect(x: 0, y: 0, width: 920, height: stackHeight + 128 + 64))
    canvas.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
    func caption(_ text: String, _ x: CGFloat, _ y: CGFloat, size: CGFloat = 12) {
        let label = PlainTextField(labelWithString: text)
        label.font = .systemFont(ofSize: size, weight: .medium)
        label.textColor = .secondaryLabelColor
        label.sizeToFit()
        label.frame = NSRect(x: x, y: y, width: min(label.frame.width, canvas.bounds.width - x - 24), height: 22)
        canvas.addSubview(label)
    }
    let height = canvas.bounds.height
    caption("hey-boss · notifications", 24, height - 44, size: 18)
    caption("344 px cards · 8 px gaps", 24, height - 80)
    var top = height - 128
    for card in keep {
        top -= card.view.frame.height
        card.view.frame.origin = NSPoint(x: 24, y: top)
        card.view.drawsSurface = false
        canvas.addSubview(card.view)
        top -= 8
    }
    precondition(keep.last!.view.frame.minY == 64, "The full stack must clear the footer")
    for pair in zip(keep, keep.dropFirst()) {
        precondition(pair.0.view.frame.minY - pair.1.view.frame.maxY == 8)
    }
    caption("Question", 400, height - 80)
    let ui = Interface(present: false)
    ui.onPresented = { _, _ in }
    let question = fixture("approval", "Release", "Which format should the report use?", options: ["Markdown", "PDF"])
    ui.add(question)
    ui.notificationCount.stringValue = "\(rows.count + 1) notifications"
    ui.stackToolbar.frame.origin = NSPoint(x: 24, y: height - 120)
    ui.stackToolbar.drawsSurface = false
    canvas.addSubview(ui.stackToolbar)
    let questionView = ui.question.contentView as! Surface
    precondition(questionView.frame.height < 180)
    questionView.drawsSurface = false
    questionView.frame.origin = NSPoint(x: 400, y: height - 86 - questionView.frame.height)
    canvas.addSubview(questionView)
    let groupRows = [rows[3], rows[0], rows[1]]
    let groupCards = groupRows.map { Card($0, open: {}, openURL: { _ in }, complete: { _, _ in }) }
    groupCards.forEach { $0.configure(grouped: true) }
    let group = ProjectGroup("Atlas", toggle: {}, clear: {})
    group.update(groupCards, expanded: false)
    group.view.drawsSurface = false
    group.view.frame.origin = NSPoint(x: 400, y: questionView.frame.minY - 102)
    canvas.addSubview(group.view)
    caption("Collapsed project · 3 updates", 400, group.view.frame.maxY + 6)
    caption("One-line update: \(Int(keep[0].view.frame.height)) px · source + action", 400, group.view.frame.minY - 44)
    caption("Summary width: 312 px · source row: 14 px", 400, group.view.frame.minY - 68)
    caption("Actual-size layout capture; solid backing replaces compositor glass.", 24, 20, size: 11)
    for view in canvas.subviews {
        precondition(canvas.bounds.contains(view.frame), "Density capture clips a view: \(view.frame)")
    }
    for (index, view) in canvas.subviews.enumerated() {
        for other in canvas.subviews.dropFirst(index + 1) {
            precondition(!view.frame.intersects(other.frame), "Density capture overlaps views")
        }
    }
    print("Compact layout (\(dark ? "dark" : "light")): update=\(Int(keep[0].view.frame.height))px, two-line=\(Int(keep[1].view.frame.height))px, question=\(Int(questionView.frame.height))px, group=\(Int(group.view.frame.height))px; source spacing and canvas bounds passed")
    guard let directory else { return }
    let window = NSWindow(contentRect: canvas.bounds, styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    defer { window.close() }
    window.contentView = canvas
    window.appearance = canvas.appearance
    canvas.layoutSubtreeIfNeeded()
    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(canvas.bounds.width), pixelsHigh: Int(canvas.bounds.height), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    bitmap.size = canvas.bounds.size
    canvas.cacheDisplay(in: canvas.bounds, to: bitmap)
    let path = URL(fileURLWithPath: directory).appendingPathComponent("balanced-\(dark ? "dark" : "light").png")
    try! bitmap.representation(using: .png, properties: [:])!.write(to: path)
    withExtendedLifetime((keep, groupCards, group, ui)) {}
}

func appearanceSnapshot(sample: Record, custom: Record, directory: String, dark: Bool) {
    let canvas = AuditCanvas(frame: NSRect(x: 0, y: 0, width: 1144, height: 688))
    canvas.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
    let heading = PlainTextField(labelWithString: "hey-boss")
    heading.font = .systemFont(ofSize: 26, weight: .semibold)
    heading.frame = NSRect(x: 32, y: 624, width: 1080, height: 36)
    canvas.addSubview(heading)
    let subtitle = PlainTextField(labelWithString: "Updates, without the clutter.")
    subtitle.font = .systemFont(ofSize: 14)
    subtitle.textColor = .secondaryLabelColor
    subtitle.frame = NSRect(x: 32, y: 596, width: 1080, height: 24)
    canvas.addSubview(subtitle)
    let kinds = ["neutral", "info", "success", "warning", "error", "error"]
    let titles = ["Review ready", "Project notes", "Checks passed", "Needs attention", "Build failed", "Custom icon, clear status"]
    let summaries = ["Three changes are ready to review.", "The latest decisions, in one place.", "All tests passed. Ready for review.", "One dependency needs your attention.", "Tests failed. Details are ready.", "Branded artwork. The failure stays visible."]
    var keep: [Card] = []
    for index in kinds.indices {
        var row = Record(taskID: UUID().uuidString, kind: "update", question: "# Report", project: "Atlas", title: titles[index], description: summaries[index], options: [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: 0, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
        if index == 5 { row.iconData = custom.iconData }
        row.severity = kinds[index]
        row.icon = index == 2 ? "test" : nil
        let card = Card(row, open: {}, openURL: { _ in }, complete: { _, _ in })
        card.configure(grouped: false)
        // The offscreen bitmap renderer cannot capture compositor-backed glass.
        // Keep the real content/layout and use the canvas's solid card backing.
        card.view.drawsSurface = false
        card.view.frame.origin = NSPoint(x: 32 + CGFloat(index % 3) * 368, y: 416 - CGFloat(index / 3) * 176)
        canvas.addSubview(card.view)
        keep.append(card)
    }
    for (index, name) in ["info", "success", "warning", "error", "build", "code", "test", "review", "deploy", "docs", "folder", "bell", "question"].enumerated() {
        var row = sample
        row.icon = name
        row.severity = ["info", "success", "warning", "error"].contains(name) ? name : "neutral"
        let x = 32 + CGFloat(index) * 84
        canvas.addSubview(IconBadge(row, frame: NSRect(x: x + 12, y: 132, width: 48, height: 48)))
        let label = PlainTextField(labelWithString: name)
        label.font = .systemFont(ofSize: 11, weight: .medium)
        label.alignment = .center
        label.frame = NSRect(x: x, y: 106, width: 72, height: 20)
        canvas.addSubview(label)
    }
    let note = PlainTextField(labelWithString: "Warning needs attention. Error means a real problem. Custom artwork keeps its colors.")
    note.font = .systemFont(ofSize: 13)
    note.textColor = .secondaryLabelColor
    note.frame = NSRect(x: 32, y: 40, width: 1080, height: 24)
    canvas.addSubview(note)
    let window = NSWindow(contentRect: canvas.bounds, styleMask: [.borderless], backing: .buffered, defer: false)
    window.contentView = canvas
    window.appearance = canvas.appearance
    canvas.layoutSubtreeIfNeeded()
    let bitmap = canvas.bitmapImageRepForCachingDisplay(in: canvas.bounds)!
    canvas.cacheDisplay(in: canvas.bounds, to: bitmap)
    let path = URL(fileURLWithPath: directory).appendingPathComponent("icons-\(dark ? "dark" : "light").png")
    try! bitmap.representation(using: .png, properties: [:])!.write(to: path)
    withExtendedLifetime(keep) {}
}

func makeAgentPreview() -> AgentsOverview {
    let now = Date().timeIntervalSince1970
    let repository: [String: Any] = ["repository_root": "/Users/example/Workspace/hey-boss", "common_dir": "/Users/example/Workspace/hey-boss/.git", "worktree": "/Users/example/Workspace/hey-boss", "branch": "main", "repository_id": "github.com/kamilio/hey-boss", "origin": "github.com/kamilio/hey-boss"]
    var worktree = repository
    worktree["worktree"] = "/Users/example/Workspace/hey-boss-activity"
    worktree["branch"] = "feature/agent-overview"
    let fixture: [String: Any] = ["host": "Fixture Mac", "observed_at": now, "warnings": [], "agents": [
        ["id": "codex-1", "pid": 101, "kind": "Codex", "cwd_source": "turn_context", "cwd": "/Users/example/Workspace/hey-boss", "session_id": "session-codex", "task": "Build a native overview of running agents and their current projects", "update": "I found the new Codex event schema. I’m checking the parser against your live session.", "activity": "Reading session events", "activity_at": now - 8, "state": "Working", "updated_at": now, "evidence": "Session file held open by this process", "git": repository],
        ["id": "codex-2", "pid": 102, "kind": "Codex", "cwd": "/Users/example/Workspace/atlas", "session_id": "session-idle", "task": "Review the release checklist", "update": "The release checks passed. The review is ready.", "activity": "Turn completed", "activity_at": now - 60, "state": "Idle", "updated_at": now - 60, "evidence": "Session file held open by this process"],
        ["id": "claude-1", "pid": 103, "kind": "Claude", "cwd": "/Users/example/Workspace/notes", "state": "Process detected", "evidence": "Running process; session unavailable"],
        ["id": "claude-2", "pid": 104, "kind": "Claude", "cwd": "/Users/example/Workspace/hey-boss-activity", "session_id": "session-claude", "task": "Make the agent activity easier to understand and group sessions by repository", "update": "I’m refining the grouped rows and checking the selection detail in light and dark appearances.", "activity": "Editing AgentsOverview.swift", "activity_at": now - 18, "state": "Working", "updated_at": now, "evidence": "PID-specific Claude metadata; process start verified", "git": worktree]
    ]]
    let overview = AgentsOverview(present: false, cli: nil)
    guard let data = try? JSONSerialization.data(withJSONObject: fixture), let local = AgentSnapshot.decode(data) else {
        overview.scanError = "Preview fixture unavailable"
        overview.rebuild()
        return overview
    }
    overview.local = local
    overview.activeOnly.state = .off
    overview.grouping.selectedSegment = 0
    overview.remote["devbox"] = AgentSnapshot(host: "devbox", observedAt: now - 60, agents: [local.agents[0]], warnings: [])
    overview.knownGroups = Set((local.agents.map { OverviewRow(agent: $0, host: "This Mac", local: true, stale: false) } + [OverviewRow(agent: local.agents[0], host: "devbox", local: false, stale: true)]).map { overview.groupIdentity($0).0 })
    overview.rebuild()
    return overview
}

func auditOverviewExpansionPersistence() {
    let suite = "local.hey-boss.test-expansion-" + UUID().uuidString
    let preferences = UserDefaults(suiteName: suite)!
    defer { preferences.removePersistentDomain(forName: suite) }
    let fixture = makeAgentPreview().local!
    let overview = AgentsOverview(present: false, cli: nil, preferences: preferences)
    overview.local = fixture
    overview.activeOnly.state = .off
    overview.rebuild()
    precondition(overview.items.count == 2 && overview.collapsedGroups.count == 2)
    let key = overview.groupIdentity(overview.rows[0]).0
    let index = overview.items.firstIndex { if case let .group(k, _, _, _) = $0 { return k == key }; return false }!
    let header = overview.tableView(overview.table, viewFor: overview.table.tableColumns[0], row: index) as! OverviewCell
    header.frame = NSRect(x: 0, y: 0, width: 900, height: 42)
    header.layoutSubtreeIfNeeded()
    let button = header.rowButtons[0]
    precondition(header.hitTest(NSPoint(x: 700, y: 20)) === button, "Entire repository header must toggle")
    button.performClick(nil)
    precondition(!overview.collapsedGroups.contains(key))
    let agentIndex = overview.items.firstIndex { if case .agent = $0 { return true }; return false }!
    let agentCell = overview.tableView(overview.table, viewFor: overview.table.tableColumns[0], row: agentIndex) as! OverviewCell
    agentCell.rowButtons[0].performClick(nil)
    overview.expansionPersistenceQueue.sync {}
    let restored = AgentsOverview(present: false, cli: nil, preferences: preferences)
    restored.local = fixture
    restored.activeOnly.state = .off
    restored.rebuild()
    precondition(!restored.collapsedGroups.contains(key) && restored.collapsedGroups.count == 1)
    precondition(restored.expandedAgents == overview.expandedAgents && !restored.expandedAgents.isEmpty)
    let restoredIndex = restored.items.firstIndex { if case let .group(k, _, _, _) = $0 { return k == key }; return false }!
    let restoredHeader = restored.tableView(restored.table, viewFor: restored.table.tableColumns[0], row: restoredIndex) as! OverviewCell
    restoredHeader.rowButtons[0].performClick(nil)
    restored.expansionPersistenceQueue.sync {}
    precondition(preferences.stringArray(forKey: "expandedRepositories")?.isEmpty == true)
    print("Passed: repository groups default collapsed, full-width header hit target, expanded repositories and inline details restored, collapsed preference saved")
}

func auditArrivalLayoutBatching() {
    let ui = Interface(present: false)
    ui.coalescesArrivalLayout = true
    let initial = ui.layoutPasses
    for index in 0..<20 {
        let row = Record(taskID: "burst-\(index)", kind: "update", question: "A short update", project: "Batch", title: "Update \(index)", description: "Ready", options: [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: Double(index), presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
        ui.add(row)
    }
    precondition(ui.cards.count == 20 && ui.layoutPasses == initial && ui.arrivalLayoutPending)
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    precondition(ui.layoutPasses == initial + 1 && !ui.arrivalLayoutPending)
    print("Passed: 20 notification arrivals coalesce to one layout, all cards retained")
}


func auditAgentControlPanel() {
    let panel = AgentControlPanel()
    var calls: [String] = []
    var done: (([String: Any]) -> Void)?
    panel.perform = { action, input, completion in
        calls.append(action)
        if action == "steer" { precondition(input["expectedTurnId"] as? String == "turn" && input["text"] as? String == "Prioritize re-enabling the goal") }
        done = completion
    }
    func reply(_ status: String) {
        done?(["ok": true, "goal": ["status": status, "objective": "Build the substantial feature"], "turnId": "turn", "canSteer": true]); done = nil
    }
    precondition(!panel.goalButton.isEnabled && !panel.sendButton.isEnabled)
    panel.connect(); panel.connect()
    precondition(calls == ["inspect"] && panel.busy)
    reply("paused")
    precondition(panel.goalButton.title == "Re-enable goal" && panel.goalButton.isEnabled)
    panel.toggleGoal(); precondition(calls.last == "enable-goal"); reply("active")
    precondition(panel.goalButton.title == "Pause goal")
    panel.toggleGoal(); precondition(calls.last == "disable-goal"); reply("paused")
    panel.editor.string = "Prioritize re-enabling the goal"
    panel.sendInstruction(); panel.sendInstruction()
    precondition(calls.filter { $0 == "steer" }.count == 1 && !panel.editor.isEditable)
    done?(["ok": false, "error": "Turn changed"]); done = nil
    precondition(panel.editor.string == "Prioritize re-enabling the goal" && !panel.sendButton.isEnabled && panel.editor.isEditable)
    panel.connect(); reply("paused"); panel.sendInstruction(); reply("paused")
    precondition(panel.editor.string.isEmpty && panel.status.stringValue == "Instruction accepted by Codex.")
    print("Passed: saved goal re-enable/pause, pending action isolation, stale-turn draft preservation, acknowledged send clears draft")
}

func auditActiveAgentFilter() {
    let preview = makeAgentPreview()
    let overview = AgentsOverview(present: false, cli: nil)
    overview.local = preview.local
    overview.remote = preview.remote
    overview.rebuild()
    precondition(overview.rows.count == 2, "Default overview must hide idle and stale sessions")
    precondition(overview.rows.allSatisfy { !$0.stale && $0.agent.state == "Working" })
    precondition(overview.activeOnly.state == .on && overview.activeOnly.superview != nil)
    precondition(overview.heading.stringValue == "Agents · 2")
    let snapshot = try! JSONSerialization.jsonObject(with: Data(overview.snapshotJSON().utf8)) as! [String: Any]
    precondition(snapshot["filter"] as? String == "active")
    overview.search.stringValue = "release"
    overview.rebuild()
    precondition(overview.rows.isEmpty && !overview.emptyState.isHidden)
    precondition(overview.emptyHint.stringValue.contains("turn off Active only"))
    overview.activeOnly.performClick(nil)
    precondition(overview.rows.count == 1 && overview.rows[0].agent.state == "Idle")
    overview.search.stringValue = ""
    overview.rebuild()
    precondition(overview.rows.count == 4)
    let allSnapshot = try! JSONSerialization.jsonObject(with: Data(overview.snapshotJSON().utf8)) as! [String: Any]
    precondition(allSnapshot["filter"] as? String == "all")
    overview.activeOnly.performClick(nil)
    precondition(overview.rows.count == 2)
    let now = Date().timeIntervalSince1970
    let data = try! JSONSerialization.data(withJSONObject: ["host": "server", "observed_at": now, "warnings": [], "agents":
        ["Working", "Waiting for input", "Idle", "Completed", "Process detected"].enumerated().map { index, state in
            ["id": "remote-\(index)", "pid": index + 200, "kind": "Claude", "session_id": "remote-\(index)", "state": state, "evidence": "Fixture"] as [String: Any]
        }])
    overview.remote = ["server": AgentSnapshot.decode(data)!]
    overview.rebuild()
    precondition(overview.rows.count == 4 && overview.rows.filter { !$0.local }.count == 2)
    overview.window.setContentSize(NSSize(width: 1000, height: 560))
    overview.window.contentView!.layoutSubtreeIfNeeded()
    precondition(overview.activeOnly.frame.minX >= overview.search.frame.maxX)
    precondition(overview.activeOnly.frame.maxX < overview.performanceLabel.frame.minX)
    print("Passed: active default, toggle, search, local/remote states, stale exclusion, JSON filter and compact layout")
}

func auditAgentOverview() -> AgentsOverview {
    auditActiveAgentFilter()
    auditArrivalLayoutBatching()
    auditAgentControlPanel()
    let closedOverview = AgentsOverview(present: true)
    precondition(closedOverview.timer == nil && !closedOverview.scanning && closedOverview.hostScans.isEmpty)
    precondition(!closedOverview.validateMenuItem(NSMenuItem(title: "Refresh agents", action: #selector(AgentsOverview.refreshNow), keyEquivalent: "")))
    closedOverview.refreshNow()
    precondition(!closedOverview.scanning && closedOverview.hostScans.isEmpty)
    closedOverview.presentWindow()
    precondition(NSApplication.shared.activationPolicy() == .regular)
    precondition(closedOverview.window.isVisible)
    closedOverview.window.close()
    precondition(NSApplication.shared.activationPolicy() == .regular, "Closing a worker window must not remove the app from the Dock")
    precondition(!closedOverview.window.isVisible && !closedOverview.scanning && closedOverview.hostScans.isEmpty)
    let ghostData = Data(#"{"host":"test","observed_at":1,"agents":[{"id":"ghost","pid":42,"kind":"Codex","state":"Unknown","evidence":"Live process only"},{"id":"empty","pid":43,"kind":"Codex","session_id":"known-empty-session","state":"Idle","evidence":"Session metadata"}],"warnings":[]}"#.utf8)
    let ghostSnapshot = AgentSnapshot.decode(ghostData)!
    let ghost = OverviewRow(agent: ghostSnapshot.agents[0], host: "test", local: true, stale: false)
    let empty = OverviewRow(agent: ghostSnapshot.agents[1], host: "test", local: true, stale: false)
    precondition(ghost.unattributed && ghost.taskLabel == "Unattributed process")
    precondition(!empty.unattributed && empty.taskLabel == "No task recorded")
    var namedAgent = empty.agent
    namedAgent.title = "Custom chat title"
    let namedRow = OverviewRow(agent: namedAgent, host: "test", local: true, stale: false)
    precondition(namedRow.chatLabel == "Custom chat title" && namedRow.taskLabel == "No task recorded")

    let ghostJSON = try! JSONSerialization.jsonObject(with: JSONEncoder().encode(ghost)) as! [String: Any]
    precondition(ghostJSON["unattributed"] as? Bool == true)

    let inventory = Data(#"{"ssh_hosts":["devbox",{"host":"kamils-macbook-pro.local"},"kamils-macbook-pro.local","-oProxyCommand=bad","bad host",{},42]}"#.utf8)
    precondition(overviewSSHHosts(inventory, excluding: "devbox") == ["kamils-macbook-pro.local"])
    precondition(overviewSSHHosts(Data("invalid".utf8), excluding: nil).isEmpty)
    precondition(overviewSSHHosts(Data(#"{"ssh_hosts":[{"host":"disabled","enabled":false},"enabled"]}"#.utf8), excluding: nil) == ["enabled"])
    precondition(overviewSSHHosts(Data(#"{"ssh_hosts":["one","two"]}"#.utf8), excluding: nil) == ["one", "two"])
    precondition(overviewRetryDelay(1) == 60)
    precondition(overviewRetryDelay(2) == 120)
    precondition(overviewRetryDelay(100) == 900)

    precondition(agentPathName("/home/server/worktree/") == "worktree")
    precondition(agentPathName("/") == "/")
    precondition(agentPathName("") == "")
    precondition(agentPathName("/unavailable-mount/project with spaces") == "project with spaces")
    let futureServer = Data(#"{"host":"server","observed_at":1e308,"agents":[],"warnings":[]}"#.utf8)
    precondition(AgentSnapshot.decode(futureServer) == nil)
    precondition(AgentSnapshot.decode(futureServer, allowClockSkew: true) != nil)
    precondition(agentRelativeTime(-Double.greatestFiniteMagnitude) == nil)
    precondition(agentRelativeTime(Double.greatestFiniteMagnitude) == "Clock ahead")
    precondition(agentRelativeTime(.nan) == nil)
    precondition(agentUpdatedTime(Double.greatestFiniteMagnitude) == "unavailable")
    let overview = makeAgentPreview()
    let local = overview.local!
    precondition(local.agents[0].sessionId == "session-codex")
    precondition(local.agents[0].cwdSource == "turn_context")
    precondition(overview.rows.count == 4 && overview.rows.allSatisfy { !$0.unattributed })
    precondition(overview.rows.filter { $0.stale }.count == 1)
    precondition(overview.rows.first { $0.stale }?.stateLabel == "Offline / stale")
    precondition(OverviewRow(agent: local.agents[0], host: "This Mac", local: true, stale: true).stateLabel == "Discovery stale")
    precondition(overview.summary.stringValue == "4 sessions")
    let remoteIndex = overview.displayIndex(for: overview.rows.first { !$0.local }!.key)!
    overview.table.selectRowIndexes(IndexSet(integer: remoteIndex), byExtendingSelection: false)
    overview.updateSelection()
    precondition(overview.grouping.superview == nil && overview.detailScroll.superview == nil)
    precondition(overview.table.numberOfColumns == 1 && overview.table.headerView == nil)
    func textIn(_ view: NSView) -> String {
        ([ (view as? NSTextField)?.stringValue ?? "" ] + view.subviews.map(textIn)).joined(separator: "\n")
    }
    let collapsed = overview.tableView(overview.table, viewFor: overview.table.tableColumns[0], row: remoteIndex) as! OverviewCell
    precondition(!textIn(collapsed).contains("PID:"))
    let same = overview.tableView(overview.table, viewFor: overview.table.tableColumns[0], row: remoteIndex)!
    precondition(same === collapsed)
    collapsed.rowButtons[0].performClick(nil)
    let expanded = overview.tableView(overview.table, viewFor: overview.table.tableColumns[0], row: remoteIndex) as! OverviewCell
    precondition(textIn(expanded).contains("PID:") && textIn(expanded).contains("session-codex"))
    precondition(overview.tableView(overview.table, heightOfRow: remoteIndex) > 112)
    expanded.rowButtons[0].performClick(nil)
    precondition(overview.tableView(overview.table, heightOfRow: remoteIndex) == 112)
    precondition(overview.performanceMetrics()["rebuild_ms"] != nil)
    do {
        let selected = overview.selectedRow()?.key
        let wasVisible = overview.window.isVisible
        let snapshot = try JSONSerialization.jsonObject(with: Data(overview.snapshotJSON().utf8)) as! [String: Any]
        precondition((snapshot["rows"] as? [[String: Any]])?.count == 4)
        precondition((snapshot["servers"] as? [[String: Any]])?.count == 1)
        precondition((snapshot["local"] as? [String: Any])?["agents"] is [[String: Any]])
        precondition(snapshot["observed_at"] != nil && snapshot["selected_row"] as? String == selected)
        precondition(overview.selectedRow()?.key == selected && overview.window.isVisible == wasVisible)
    } catch { preconditionFailure("Overview snapshot failed: \(error)") }
    overview.search.stringValue = "release"
    overview.rebuild()
    precondition(overview.rows.count == 1 && overview.rows[0].agent.state == "Idle")
    overview.search.stringValue = ""
    precondition(overview.filter.superview == nil && overview.summary.superview == nil)
    overview.search.stringValue = "Claude"
    overview.rebuild()
    precondition(overview.rows.count == 1 && overview.rows.allSatisfy { $0.agent.kind == "Claude" })
    overview.search.stringValue = ""
    overview.rebuild()
    let membership = Set(overview.rows.map(\.key))
    let selectedKey = overview.rows.first { $0.local && $0.agent.kind == "Claude" && $0.agent.sessionId != nil }!.key
    overview.table.selectRowIndexes(IndexSet(integer: overview.displayIndex(for: selectedKey)!), byExtendingSelection: false)
    for mode in 0..<3 {
        overview.grouping.selectedSegment = mode
        overview.rebuild()
        precondition(Set(overview.rows.map(\.key)) == membership)
        precondition(overview.selectedRow()?.key == selectedKey)
        let groupCount = overview.items.filter { if case .group = $0 { return true }; return false }.count
        precondition(groupCount == 2)
    }
    overview.grouping.selectedSegment = 0
    overview.rebuild()
    overview.collapsedGroups = Set(overview.rows.map { overview.groupIdentity($0).0 })
    overview.rebuild()
    precondition(Set(overview.rows.map(\.key)) == membership && overview.items.count == 2)
    precondition(overview.selectedRow() == nil)
    overview.collapsedGroups = []
    overview.search.stringValue = "no-such-agent"
    overview.rebuild()
    precondition(!overview.emptyState.isHidden && overview.emptyTitle.stringValue == "No matching agents")
    overview.search.stringValue = ""
    overview.rebuild()
    precondition(overview.emptyState.isHidden)
    precondition(overview.tableView(overview.table, viewFor: overview.table.tableColumns.first, row: -1) == nil)
    precondition(!overview.tableView(overview.table, shouldSelectRow: 10000))
    for _ in 0..<20 {
        overview.rebuild()
        for index in overview.items.indices {
            _ = overview.tableView(overview.table, viewFor: overview.table.tableColumns[0], row: index)
        }
    }
    let metrics = overview.performanceMetrics()
    print("Overview warm benchmark: \(metrics)")
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent("hey-boss-screenshots")
    do { try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true) }
    catch { print("Screenshot export unavailable: \(error)"); return overview }
    if let row = overview.rows.first(where: { $0.local && $0.agent.kind == "Codex" && $0.agent.sessionId != nil }) {
        let panel = overview.controlPanel(for: row)
        panel.perform = { _, _, completion in completion(["ok": true, "goal": ["status": "paused", "objective": "Build a native overview of running agents and their current projects"], "turnId": "preview-turn", "canSteer": true]) }
        panel.connect()
        overview.expandedAgents.insert(row.key); overview.rebuild()
    }
    for dark in [false, true] {
        overview.window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        let canvas = overview.window.contentView!
        canvas.layoutSubtreeIfNeeded()
        for row in overview.items.indices {
            for column in 0..<overview.table.numberOfColumns { _ = overview.table.view(atColumn: column, row: row, makeIfNecessary: true) }
        }
        let bitmap = canvas.bitmapImageRepForCachingDisplay(in: canvas.bounds)!
        canvas.cacheDisplay(in: canvas.bounds, to: bitmap)
        if let data = bitmap.representation(using: .png, properties: [:]) {
            do { try data.write(to: directory.appendingPathComponent("agents-overview-\(dark ? "dark" : "light").png")) }
            catch { print("Screenshot export failed: \(error)") }
        }
    }
    print("Passed: agent snapshot decoding, live/stale counts, task search, agent filter, safe remote selection")
    return overview
}

final class AgentPreviewController: NSObject {
    let overview: AgentsOverview
    let originalLocal: AgentSnapshot
    let originalRemote: [String: AgentSnapshot]
    var usingLive = false
    var serverConnected = false
    var pulse: Timer?
    init(_ overview: AgentsOverview) {
        self.overview = overview
        originalLocal = overview.local ?? AgentSnapshot(host: "Preview", observedAt: Date().timeIntervalSince1970, agents: [], warnings: [])
        originalRemote = overview.remote
        super.init()
        pulse = Timer.scheduledTimer(withTimeInterval: 15, repeats: true) { [weak self] _ in self?.renewFixture() }
    }
    deinit { pulse?.invalidate() }
    func renewFixture() {
        if usingLive { overview.scanLocal(); return }
        guard let local = overview.local else { return }
        let now = Date().timeIntervalSince1970
        overview.local = AgentSnapshot(host: local.host, observedAt: now, agents: local.agents, warnings: local.warnings)
        if serverConnected {
            overview.remote = overview.remote.mapValues { AgentSnapshot(host: $0.host, observedAt: now, agents: $0.agents, warnings: $0.warnings) }
        }
        overview.rebuild()
    }
    func menu() -> NSMenu {
        let root = NSMenu()
        let app = NSMenu(title: "Preview")
        app.addItem(withTitle: "Quit preview", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        let appItem = root.addItem(withTitle: "Preview", action: nil, keyEquivalent: "")
        appItem.submenu = app
        let view = NSMenu(title: "View")
        for (title, action) in [("Light appearance", #selector(light)), ("Dark appearance", #selector(dark)), ("System appearance", #selector(system)), ("Compact window", #selector(compact)), ("Default window", #selector(normal)), ("Wide window", #selector(wide))] {
            view.addItem(withTitle: title, action: action, keyEquivalent: "").target = self
        }
        root.addItem(withTitle: "View", action: nil, keyEquivalent: "").submenu = view
        let fixtures = NSMenu(title: "Fixtures")
        for (title, action) in [("Standard sessions", #selector(standard)), ("Connected server", #selector(connected)), ("Many sessions", #selector(many)), ("No sessions", #selector(empty)), ("Live local sessions", #selector(live))] {
            fixtures.addItem(withTitle: title, action: action, keyEquivalent: "").target = self
        }
        root.addItem(withTitle: "Fixtures", action: nil, keyEquivalent: "").submenu = fixtures
        return root
    }
    @objc func live() {
        usingLive = true
        overview.local = nil
        overview.search.stringValue = ""
        overview.filter.selectedSegment = 0
        overview.remote = [:]
        overview.scanLocal()
    }
    @objc func light() { overview.window.appearance = NSAppearance(named: .aqua) }
    @objc func dark() { overview.window.appearance = NSAppearance(named: .darkAqua) }
    @objc func system() { overview.window.appearance = nil }
    @objc func compact() { overview.window.setContentSize(NSSize(width: 1000, height: 620)) }
    @objc func normal() { overview.window.setContentSize(NSSize(width: 1240, height: 760)) }
    @objc func wide() { overview.window.setContentSize(NSSize(width: 1440, height: 850)) }
    @objc func standard() {
        usingLive = false
        serverConnected = false
        overview.local = AgentSnapshot(host: originalLocal.host, observedAt: Date().timeIntervalSince1970, agents: originalLocal.agents, warnings: originalLocal.warnings)
        overview.remote = originalRemote
        overview.search.stringValue = ""
        overview.filter.selectedSegment = 0
        overview.collapsedGroups = []
        overview.rebuild()
    }
    @objc func connected() {
        standard()
        serverConnected = true
        for snapshot in originalRemote.values { overview.receive(snapshot) }
    }
    @objc func many() {
        standard()
        var agents: [AgentInfo] = []
        guard !originalLocal.agents.isEmpty else { empty(); return }
        for index in 0..<24 {
            let source = originalLocal.agents[index % originalLocal.agents.count]
            agents.append(AgentInfo(id: "many-\(index)", pid: UInt32(100 + index), kind: source.kind, cwd: source.cwd, sessionId: source.sessionId == nil ? nil : "session-\(index)", task: source.task, activity: source.activity, state: source.state, updatedAt: source.updatedAt, evidence: source.evidence, update: source.update, activityAt: source.activityAt, git: source.git))
        }
        overview.local = AgentSnapshot(host: originalLocal.host, observedAt: Date().timeIntervalSince1970, agents: agents, warnings: [])
        overview.rebuild()
    }
    @objc func empty() {
        usingLive = false
        serverConnected = false
        overview.search.stringValue = ""
        overview.local = AgentSnapshot(host: originalLocal.host, observedAt: Date().timeIntervalSince1970, agents: [], warnings: [])
        overview.remote = [:]
        overview.rebuild()
    }
}

/// Exercise the native receiver, including its durable state and actual reply sockets.
func auditNotificationReplay() {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("hb-replay-" + UUID().uuidString)
    try! FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try! FileManager.default.removeItem(at: root) }
    let path = root.appendingPathComponent("history.db").path
    var store = try! Store(path)
    var shown: [Record] = []
    store.show = { shown.append($0) }
    func request(_ kind: String, id: String? = nil) -> [String: Any] {
        var body: [String: Any] = ["command":kind, "project":"Replay test", "title":"Delivery recovered", "question":"Synthetic notification", "description":"Connection restored", "source_host":"audit.test", "sync":false]
        if let id { body["task_id"] = id }
        if kind == "ask" { body["options"] = ["Continue", "Cancel"] }
        return body
    }
    func call(_ body: [String: Any]) -> [String: Any] {
        var peers: [Int32] = [0, 0]
        precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &peers) == 0)
        let reader = FileHandle(fileDescriptor: peers[1], closeOnDealloc: true)
        let decoded = try! JSONDecoder().decode(Request.self, from: JSONSerialization.data(withJSONObject: body))
        store.handle(decoded, Reply(peers[0]))
        return try! JSONSerialization.jsonObject(with: reader.readToEnd()!) as! [String: Any]
    }
    for kind in ["alert", "update", "ask"] {
        let body = request(kind, id: "stable-" + kind)
        let first = call(body)
        let count = shown.count
        let retry = call(body)
        precondition(first["task_id"] as? String == retry["task_id"] as? String, "Replay must return the original native ID")
        precondition(shown.count == count, "Replay must not present again")
        precondition((try! store.database.pendingCount()) == count)
    }
    let question = request("ask", id: "stable-ask")
    let id = call(question)["task_id"] as! String
    try! store.finish(id, "Continue")
    // Mutable record fields (answers, comments, presentation) are not the request fingerprint.
    var sync = question; sync["sync"] = true
    precondition(call(sync)["result"] as? String == "Continue")
    store = try! Store(path)
    store.show = { shown.append($0) }
    precondition(call(question)["task_id"] as? String == id)
    precondition(call(sync)["result"] as? String == "Continue")
    for kind in ["alert", "update"] { _ = call(request(kind, id: "stable-" + kind)) }
    precondition(shown.count == 3, "Restarted receiver must not present a replay")
    var restored: [String] = []
    store.show = { restored.append($0.taskID) }; store.restore()
    precondition(restored.count == 2, "Startup restores only the two pending notices")
    for kind in ["alert", "update", "ask"] { _ = call(request(kind, id: "stable-" + kind)) }
    precondition(restored.count == 2, "Replays must not present another copy after startup restoration")
    store.show = { shown.append($0) }
    let cancelled = request("ask", id: "cancelled")
    let cancelledID = call(cancelled)["task_id"] as! String
    try! store.dismissRecords([cancelledID])
    store = try! Store(path); store.show = { shown.append($0) }
    var cancelledSync = cancelled; cancelledSync["sync"] = true
    precondition(call(cancelledSync)["status"] as? String == "cancelled")
    // Every user-visible payload change must conflict, including identity/provenance.
    for (key, value) in [("question", "Changed"), ("title", "Changed"), ("project", "Other"), ("description", "Changed"), ("command", "update"), ("icon", "warning"), ("document_name", "other.md")] {
        var conflict = question; conflict[key] = value
        precondition(call(conflict)["status"] as? String == "error", "Conflicting \(key) accepted")
    }
    for (key, value) in [("options", ["Different"] as Any), ("autoclose", 10 as Any), ("comments_enabled", true as Any), ("issue", ["project":"named:Other", "number":1] as Any)] {
        var conflict = question; conflict[key] = value
        precondition(call(conflict)["status"] as? String == "error", "Conflicting \(key) accepted")
    }
    var other = question; other["source_host"] = "second.test"
    precondition(call(other)["task_id"] as? String != id, "Different machines must have separate ID scopes")
    let anonymous = request("alert")
    precondition(call(anonymous)["task_id"] as? String != call(anonymous)["task_id"] as? String)
    let beforeFailure = try! store.database.inboxRows().count
    try! store.database.execute("CREATE TRIGGER reject_receipt BEFORE INSERT ON notification_receipts WHEN NEW.request_id='atomic-failure' BEGIN SELECT RAISE(ABORT, 'synthetic receipt failure'); END")
    precondition(call(request("alert", id: "atomic-failure"))["status"] as? String == "error")
    precondition((try! store.database.inboxRows().count) == beforeFailure && shown.count == beforeFailure)
    try! store.database.execute("DROP TRIGGER reject_receipt")
    _ = call(request("alert", id: "atomic-failure"))
    precondition((try! store.database.inboxRows().count) == beforeFailure + 1)
    // Two synchronous retries both receive the answer from one presentation.
    var pendingSync = request("ask", id: "sync-waiters"); pendingSync["sync"] = true
    var readers: [FileHandle] = []
    for _ in 0..<2 {
        var peers: [Int32] = [0, 0]
        precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &peers) == 0)
        readers.append(FileHandle(fileDescriptor: peers[1], closeOnDealloc: true))
        store.handle(try! JSONDecoder().decode(Request.self, from: JSONSerialization.data(withJSONObject: pendingSync)), Reply(peers[0]))
    }
    let waitingID = shown.last!.taskID
    precondition(store.waiters[waitingID]?.count == 2)
    try! store.finish(waitingID, "Continue")
    for reader in readers {
        let answer = try! JSONSerialization.jsonObject(with: reader.readToEnd()!) as! [String: Any]
        precondition(answer["task_id"] as? String == waitingID && answer["result"] as? String == "Continue")
    }
    let beforeSecret = try! store.database.inboxRows().count
    _ = call(["command":"secret", "task_id":"never-persist", "sync":false])
    precondition((try! store.database.inboxRows().count) == beforeSecret)
    print("Passed: native replay IDs, no duplicate presentation, restart, terminal questions, payload conflicts, source isolation, anonymous callers and secret exclusion")
    auditCompanionNotificationReplay()
}

func auditCompanionNotificationReplay(page: Bool = false) {
    // Keep Unix socket paths below Darwin's 104-byte limit.
    let root = URL(fileURLWithPath: "/tmp/hb-replay-" + UUID().uuidString)
    try! FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try! FileManager.default.removeItem(at: root) }
    try! Data("1".utf8).write(to: root.appendingPathComponent("bridge-protocol"))
    func socketAt(_ name: String, listen: Bool = false) -> Int32 {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        precondition(fd >= 0)
        var address = sockaddr_un(); address.sun_family = sa_family_t(AF_UNIX)
        let path = root.appendingPathComponent(name).path.utf8CString
        precondition(path.count <= MemoryLayout.size(ofValue: address.sun_path))
        withUnsafeMutableBytes(of: &address.sun_path) { target in target.copyBytes(from: path.map { UInt8(bitPattern: $0) }) }
        let result = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                listen ? Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) : Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        if result != 0 { Darwin.close(fd); return -1 }
        if listen { precondition(Darwin.listen(fd, 16) == 0) }
        return fd
    }
    func write(_ fd: Int32, _ body: [String: Any]) {
        let data = try! JSONSerialization.data(withJSONObject: body)
        let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: false)
        try! handle.write(contentsOf: data)
        precondition(shutdown(fd, SHUT_WR) == 0)
    }
    let broker = Process()
    broker.executableURL = URL(fileURLWithPath: ProcessInfo.processInfo.environment["HEY_BOSS_CLI_PATH"] ?? FileManager.default.currentDirectoryPath + "/target/debug/hey-boss")
    broker.arguments = ["companion", "serve", "--state", root.path]
    try! broker.run()
    defer { broker.terminate(); broker.waitUntilExit() }
    let deadline = Date().addingTimeInterval(10)
    while true {
        let fd = socketAt("daemon.sock")
        if fd >= 0 { Darwin.close(fd); break }
        precondition(Date() < deadline, "Companion startup timed out")
        Thread.sleep(forTimeInterval: 0.02)
    }
    for kind in ["alert", "update", "ask"] {
        let fd = socketAt("daemon.sock"); precondition(fd >= 0)
        write(fd, ["command":kind, "project":"Replay test", "title":kind.capitalized + " recovered", "question":"Synthetic lost acknowledgement", "description":"One notification after reconnect", "sync":false, "page":page])
        let response = try! JSONSerialization.jsonObject(with: readRequest(fd)) as! [String: Any]
        Darwin.close(fd)
        precondition(response["task_id"] != nil && response["status"] as? String != "error")
    }
    let listener = socketAt("bridge.sock", listen: true); precondition(listener >= 0)
    defer { Darwin.close(listener) }
    let path = root.appendingPathComponent("history.db").path
    var store = try! Store(path)
    store.notificationsMuted = { page }
    var shown: [Record] = []
    store.show = { shown.append($0) }
    var accepted: [String: String] = [:], attempts: [String: Int] = [:]
    while attempts.values.reduce(0, +) < 6 {
        var ready = pollfd(fd: listener, events: Int16(POLLIN), revents: 0)
        precondition(poll(&ready, 1, 15000) > 0, "Companion replay timed out")
        let fd = accept(listener, nil, nil); precondition(fd >= 0)
        let data = try! readRequest(fd)
        let request = try! JSONDecoder().decode(Request.self, from: data)
        if request.command == "agents_snapshot" {
            write(fd, ["task_id":"agents", "status":"ok"]); Darwin.close(fd); continue
        }
        var peers: [Int32] = [0, 0]
        precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &peers) == 0)
        store.handle(request, Reply(peers[0]))
        let response = try! JSONSerialization.jsonObject(with: readRequest(peers[1])) as! [String: Any]
        Darwin.close(peers[1])
        if ["alert", "update", "ask"].contains(request.command) {
            let key = request.task_id!
            attempts[key, default: 0] += 1
            let id = response["task_id"] as! String
            if attempts[key] == 1 {
                accepted[key] = id
                // Native persistence succeeded, but the bridge disconnects without its ack.
                Darwin.close(fd)
                // Reopen the actual native DB before the broker's reconnect/retry.
                store = try! Store(path); store.show = { shown.append($0) }
                store.notificationsMuted = { page }
                continue
            }
            precondition(id == accepted[key], "Broker retry created another native record")
        }
        write(fd, response); Darwin.close(fd)
    }
    precondition(attempts.count == 3 && attempts.values.allSatisfy { $0 == 2 })
    precondition(shown.count == 3 && Set(shown.map(\.taskID)).count == 3)
    precondition(shown.allSatisfy { ($0.page == true) == page })
    precondition((try! store.database.inboxRows()).count == 3)
    print("Passed: real companion → native Store, lost ack, reconnect and receiver restart: three commands, three rows, three presentations")
    auditReplayPresentation(shown, store: store)
}

func auditReplayPresentation(_ rows: [Record], store: Store) {
    let directory = ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_SNAPSHOT_DIR"]
    let ui = Interface(present: false)
    ui.notificationsMuted = { rows.allSatisfy { $0.page == true } }
    defer { ui.stack.orderOut(nil); ui.question.orderOut(nil) }
    ui.onComplete = { id, answer in try! store.finish(id, answer); ui.remove(id) }
    ui.onPresented = { store.presented($0, $1) }
    for row in rows { ui.add(row) }
    ui.layout()
    RunLoop.main.run(until: Date().addingTimeInterval(0.15))
    precondition(ui.cards.count == 2 && ui.current?.kind == "prompt")
    func snapshot(_ name: String, _ window: NSWindow) {
        guard let directory else { return }
        try! FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
        // Offscreen AppKit cannot capture compositor glass. Render the same native
        // controls and geometry over the solid backing used by other visual audits.
        let preview = Interface(present: false)
        for card in ui.cards { preview.add(card.row) }
        if let current = ui.current { preview.add(current) }
        preview.layout()
        let source = window === ui.stack ? preview.stack : preview.question
        let canvas = AuditCanvas(frame: source.contentView!.bounds.insetBy(dx: -8, dy: -8))
        canvas.setBoundsOrigin(.zero)
        let surfaces: [Surface] = window === ui.stack ? [preview.stackToolbar] + preview.cards.map(\.view) : [source.contentView as! Surface]
        let frames = surfaces.map { $0.convert($0.bounds, to: source.contentView) }
        for (surface, frame) in zip(surfaces, frames) {
            surface.drawsSurface = false; canvas.addSubview(surface)
            surface.frame = frame.offsetBy(dx: 8, dy: 8)
            precondition(canvas.bounds.contains(surface.frame), "Replay capture clips a control")
        }
        let capture = NSWindow(contentRect: canvas.bounds, styleMask: [.borderless], backing: .buffered, defer: false)
        capture.isReleasedWhenClosed = false; capture.contentView = canvas; capture.appearance = window.appearance
        defer { capture.close(); preview.stack.orderOut(nil); preview.question.orderOut(nil) }
        canvas.layoutSubtreeIfNeeded()
        let bitmap = canvas.bitmapImageRepForCachingDisplay(in: canvas.bounds)!
        canvas.cacheDisplay(in: canvas.bounds, to: bitmap)
        try! bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: directory).appendingPathComponent(name + ".png"))
    }
    for dark in [false, true] {
        let mode = dark ? "dark" : "light"
        ui.stack.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        ui.question.appearance = ui.stack.appearance
        snapshot("replay-notifications-" + mode, ui.stack)
        snapshot("replay-question-" + mode, ui.question)
    }
    ui.field!.stringValue = "Synthetic answer"
    ui.buttons[0].performClick(nil)
    RunLoop.main.run(until: Date().addingTimeInterval(0.15))
    precondition(ui.current == nil)
    precondition((try! store.database.get(rows.last!.taskID)).result == "Synthetic answer")
    var approval = Record(taskID: "visual-approval", kind: "approval", question: "Choose the synthetic test result", project: "Replay test", title: "Reconnected question", description: "Only one approval is shown", options: ["Continue", "Cancel"], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: Date().timeIntervalSince1970, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
    approval.page = rows.first?.page
    try! store.database.save(approval); ui.add(approval)
    for dark in [false, true] {
        ui.question.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        snapshot("replay-approval-" + (dark ? "dark" : "light"), ui.question)
    }
    ui.buttons.first { $0.title == "Continue" }!.performClick(nil)
    RunLoop.main.run(until: Date().addingTimeInterval(0.15))
    precondition(ui.current == nil && (try! store.database.get(approval.taskID)).result == "Continue")
    print("Passed: native notification stack, free-text and approval interaction, light/dark presentation and terminal dismissal")
}

func auditResilience(root: URL) {
    precondition(notificationTimerInterval(autoclose: 10, expiry: nil, now: 100) == 10)
    precondition(notificationTimerInterval(autoclose: 10, expiry: 105, now: 100) == 5)
    precondition(notificationTimerInterval(autoclose: 10, expiry: Double.greatestFiniteMagnitude, now: 100) == 10)
    precondition(notificationTimerInterval(autoclose: 10, expiry: .infinity, now: 100) == 10)
    precondition(notificationTimerInterval(autoclose: 10, expiry: -Double.greatestFiniteMagnitude, now: 100) == 0.001)
    for invalid in [Double.nan, Double.infinity, -1, 0, Double.greatestFiniteMagnitude] {
        precondition(notificationTimerInterval(autoclose: invalid, expiry: nil, now: 100) == nil)
    }

    let store = try! Store(root.appendingPathComponent("resilience.db").path)
    let bannerStore = try! Store(root.appendingPathComponent("banner-ux.db").path)
    let bannerJSON: [String: Any] = ["taskID":"hidden-banner","kind":"update","question":"Report","project":"Synthetic","title":"Unread update","description":"Ready","options":[],"createdAt":100,"status":"pending"]
    var hidden = try! JSONDecoder().decode(Record.self, from: JSONSerialization.data(withJSONObject: bannerJSON))
    hidden.bannerHidden = true // Legacy unread banner hidden by the old timer.
    try! bannerStore.database.save(hidden)
    var restoredBanners: [String] = []
    bannerStore.show = { restoredBanners.append($0.taskID) }
    bannerStore.restore()
    precondition(restoredBanners == [hidden.taskID])
    let retained = try! bannerStore.database.get(hidden.taskID)
    precondition(retained.status == "pending" && retained.bannerHidden == false)
    let inboxUI = Interface(present: false)
    inboxUI.add(retained)
    precondition(inboxUI.cards.count == 1 && inboxUI.cards[0].timer == nil)
    func rejected(_ body: [String: Any]) {
        var fds: [Int32] = [0, 0]
        precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
        let request = try! JSONDecoder().decode(Request.self, from: JSONSerialization.data(withJSONObject: body))
        store.handle(request, Reply(fds[0]))
        let handle = FileHandle(fileDescriptor: fds[1], closeOnDealloc: true)
        let response = try! JSONDecoder().decode([String: String].self, from: handle.readToEnd()!)
        precondition(response["status"] == "error")
    }
    rejected(["command": "unknown", "sync": false])
    rejected(["command": "status", "task_id": "missing", "sync": false])
    rejected(["command": "alert", "sync": false])
    rejected(["command": "alert", "project": "Test", "title": "Test", "question": "Test", "link_url": "javascript:alert(1)", "link_label": "Open", "sync": false])
    rejected(["command": "alert", "project": "Test", "title": "Test", "question": "Test", "autoclose": -1, "sync": false])
    rejected(["command": "alert", "project": "Test", "title": "Test", "question": "Test", "autoclose": 1e308, "sync": false])
    try! store.database.execute("INSERT INTO dialogs VALUES ('damaged', 'pending', 'broken json')")
    precondition((try! store.database.pending()).isEmpty)
    do { _ = try Database(root.appendingPathComponent("missing/history.db").path); preconditionFailure("Unavailable storage accepted") } catch {}
    let incompatible = root.appendingPathComponent("incompatible.db").path
    do {
        let seed = try! Database(incompatible)
        try! seed.execute("DROP TABLE dialogs; CREATE TABLE dialogs (unexpected TEXT)")
    }
    for _ in 0..<3 {
        do { _ = try Database(incompatible); preconditionFailure("Incompatible schema accepted") } catch {}
    }
    try! store.database.execute("PRAGMA query_only=ON")
    rejected(["command": "alert", "project": "Test", "title": "Test", "question": "Test", "sync": false])
    try! store.database.execute("PRAGMA query_only=OFF")
    precondition((try! store.database.pending()).isEmpty)
    print("Passed: malformed requests, missing tasks, unsafe links, invalid expiry, damaged history, unavailable/read-only storage remain recoverable")
}

func auditConnectionSettings() {
    for domain in ["", ".quora.net", "quora..net", "-quora.net", "quora-.net", "quora net", String(repeating: "a", count: 64) + ".net"] {
        precondition(ConnectionPreferences(host: "devbox", vpnDomain: domain, enabled: true).validationError != nil)
    }
    precondition(ConnectionPreferences(host: "user@devbox", vpnDomain: "quora.net.", enabled: true).validationError == nil)
    precondition(connectionStateDescription(["state":"waiting-for-vpn"], host: "devbox") == "Waiting for VPN")
    precondition(connectionStateDescription(["state":"backoff","retry_at":Double.greatestFiniteMagnitude], host: "devbox").contains("waiting to retry"))
    var calls: [ConnectionPreferences] = []
    var completion: ((String?) -> Void)?
    let settings = ConnectionSettingsController(preferences: ConnectionPreferences(host: "devbox", vpnDomain: "quora.net", enabled: true), status: "Automatic connection paused", preview: true) { preferences, callback in
        calls.append(preferences); completion = callback
    }
    settings.host.stringValue = "-bad"
    settings.save()
    precondition(calls.isEmpty && !settings.saving)
    settings.host.stringValue = " devbox "
    settings.vpnDomain.stringValue = " Quora.NET "
    settings.save()
    settings.save()
    precondition(calls.count == 1 && settings.saving && !settings.saveButton.isEnabled)
    precondition(calls[0].host == "devbox" && calls[0].vpnDomain == "quora.net")
    completion?("Synthetic save failure")
    precondition(!settings.saving && settings.saveButton.isEnabled && settings.feedback.stringValue == "Synthetic save failure")
    settings.save()
    completion?(nil)
    completion?("Late duplicate callback")
    precondition(settings.feedback.stringValue.contains("no changes applied"))
    print("Passed: native connection settings, DNS/host validation, normalized inputs, save errors, duplicate callbacks, preview isolation")
}

func waitForNativeReader(_ reader: Preview) {
    let deadline = Date().addingTimeInterval(25)
    while !reader.readerLoaded && Date() < deadline { RunLoop.main.run(until:Date().addingTimeInterval(0.01)) }
    precondition(reader.readerLoaded,"Native Markdown renderer did not finish")
}
func auditNativeMarkdownReader(sample: Record) {
    let source = "---\ntitle: Hidden\n---\n\n# Report 🌍\n\n| Name | Value |\n|---|---:|\n| alpha | 2 |\n\n- [x] **Done**\n  - Nested\n\n> [!WARNING]\n> Review carefully.\n\n```rust\nlet first = true;\n    let next = 2;\n```\n\n<script>literal HTML</script>"
    let row = Record(taskID:"native-reader-audit",kind:"update",question:source,project:"Synthetic",title:"Native Markdown",description:"Fixture",options:[],autoclose:nil,linkURL:nil,linkLabel:nil,createdAt:0,presentedAt:nil,expiresAt:nil,status:"pending",result:nil,origin:nil)
    let start = ProcessInfo.processInfo.systemUptime
    let reader = Preview(row,openURL:{ _ in })
    func snapshot(_ name: String) {
        guard let directory = ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_SNAPSHOT_DIR"], let view = reader.contentView else { return }
        try! FileManager.default.createDirectory(atPath:directory,withIntermediateDirectories:true)
        view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
        let bitmap = view.bitmapImageRepForCachingDisplay(in:view.bounds)!
        view.cacheDisplay(in:view.bounds,to:bitmap)
        try! bitmap.representation(using:.png,properties:[:])!.write(to:URL(fileURLWithPath:directory).appendingPathComponent(name+".png"))
    }
    precondition(reader.loadingView != nil && reader.text.string.isEmpty && reader.fallbackScroll!.isHidden,"Never expose an intermediate document")
    reader.makeKeyAndOrderFront(nil)
    snapshot("native-markdown-loading")
    waitForNativeReader(reader)
    precondition(reader.loadingView == nil && !reader.fallbackScroll!.isHidden)
    precondition(!reader.contentView!.subviews.contains { $0 is WKWebView },"Document rendering must be native")
    let storage = reader.text.textStorage!
    precondition(storage.string.contains("Report 🌍") && !storage.string.contains("title: Hidden"))
    precondition(storage.string.contains("☑\tDone") && storage.string.contains("Nested"))
    precondition(storage.string.contains("Warning") && storage.string.contains("<script>literal HTML</script>"))
    var table = false, highlighted = false
    storage.enumerateAttributes(in:NSRange(location:0,length:storage.length)) { attributes, _, _ in
        if (attributes[.paragraphStyle] as? NSParagraphStyle)?.textBlocks.contains(where: { $0 is NSTextTableBlock }) == true { table = true }
        if attributes[.nativeCode] as? Bool == true && attributes[.foregroundColor] as? NSColor == .systemPurple { highlighted = true }
    }
    precondition(table && highlighted,"Native tables and highlighted code must retain their structure")
    for phrase in ["Review carefully.", "let first = true;"] {
        let range = (storage.string as NSString).range(of:phrase)
        let layout = reader.text.layoutManager!, container = reader.text.textContainer!
        let glyphs = layout.glyphRange(forCharacterRange:range,actualCharacterRange:nil)
        let bounds = layout.boundingRect(forGlyphRange:glyphs,in:container)
        precondition(bounds.height < 50 && bounds.width > 80,"Native blocks must use the available width: \(phrase) \(bounds)")
    }
    reader.appearance = NSAppearance(named:.aqua); snapshot("native-markdown-light")
    reader.appearance = NSAppearance(named:.darkAqua); snapshot("native-markdown-dark")
    let pasteboard = NSPasteboard(name:.init("hey-boss-copy-audit-"+UUID().uuidString))
    defer { pasteboard.releaseGlobally() }
    reader.text.copyPasteboard = pasteboard
    reader.makeFirstResponder(reader.text)
    let code = "let first = true;\n    let next = 2;\n"
    reader.text.setSelectedRange((storage.string as NSString).range(of:code))
    let copy = NSEvent.keyEvent(with:.keyDown,location:.zero,modifierFlags:.command,timestamp:0,windowNumber:reader.windowNumber,context:nil,characters:"c",charactersIgnoringModifiers:"c",isARepeat:false,keyCode:8)!
    precondition(reader.performKeyEquivalent(with:copy),"Command-C must reach the native selection")
    precondition(pasteboard.string(forType:.string) == code,"Copy changed code indentation or newlines")
    precondition(pasteboard.data(forType:.rtf) != nil,"Copy must include rich text")
    let cells = "Name\nValue\nalpha\n2\n"
    reader.text.setSelectedRange((storage.string as NSString).range(of:cells))
    reader.text.copy(nil)
    precondition(pasteboard.string(forType:.string) == "Name\tValue\nalpha\t2\n","Table selection must paste as TSV")
    reader.text.selectAll(nil); reader.text.copy(nil)
    precondition(pasteboard.string(forType:.string)?.contains("Report 🌍") == true && pasteboard.string(forType:.string)?.contains(code) == true,"Copy must cross every document block")
    for width: CGFloat in [720,420] {
        reader.setContentSize(NSSize(width:width,height:640)); reader.contentView!.layoutSubtreeIfNeeded()
        reader.text.layoutManager!.ensureLayout(for:reader.text.textContainer!)
        precondition(reader.text.frame.width <= reader.fallbackScroll!.contentSize.width+1)
    }
    let savedSelection = reader.text.selectedRange()
    RunLoop.main.run(until:Date().addingTimeInterval(0.2))
    precondition(reader.text.selectedRange() == savedSelection,"A late renderer must not replace an active selection")
    print("Passed: native blocks, stable loading, tables, code, lists, callouts, literal HTML, Unicode and rich copy, exact code whitespace, TSV, cross-block selection; ready_ms=\(Int((ProcessInfo.processInfo.systemUptime-start)*1000))")
    reader.close()
}

func auditLargeNativeMarkdownReader(sample: Record) {
    let paragraph = String(repeating:"Readable **native text** with Unicode 🌍 and selectable words. ",count:6)
    let source = (0..<3000).map { "## Section \($0)\n\n\(paragraph)\n\n" }.joined() + "## Final section\n\nFinal selectable text 🌍.\n"
    let row = Record(taskID:"native-large-audit",kind:"update",question:source,project:"Synthetic",title:"Large document",description:"",options:[],autoclose:nil,linkURL:nil,linkLabel:nil,createdAt:0,presentedAt:nil,expiresAt:nil,status:"pending",result:nil,origin:nil)
    precondition(row.question.utf8.count > 1024*1024)
    let start = ProcessInfo.processInfo.systemUptime
    let reader = Preview(row,openURL:{ _ in })
    precondition(ProcessInfo.processInfo.systemUptime-start < 1,"Opening a large document must not block the UI")
    precondition(reader.loadingView != nil && reader.text.string.isEmpty)
    reader.makeKeyAndOrderFront(nil)
    var ticks = 0
    let timer = Timer.scheduledTimer(withTimeInterval:0.02,repeats:true) { _ in ticks += 1 }
    waitForNativeReader(reader)
    timer.invalidate()
    let renderTime = ProcessInfo.processInfo.systemUptime-start
    precondition(ticks > 0,"Loading must allow the main run loop to process events")
    precondition(!reader.subtitle.contains("Plain text"),"Large Markdown must render natively without fallback")
    let final = "Final selectable text 🌍."
    let range = (reader.text.string as NSString).range(of:final)
    precondition(range.location != NSNotFound,"Large documents must not be truncated")
    reader.text.setSelectedRange(range)
    reader.text.scrollRangeToVisible(range)
    let pasteboard = NSPasteboard(name:.init("hey-boss-large-copy-"+UUID().uuidString))
    defer { pasteboard.releaseGlobally(); reader.close() }
    reader.text.copyPasteboard = pasteboard; reader.text.copy(nil)
    precondition(pasteboard.string(forType:.string) == final)
    print("Passed: large native document bytes=\(row.question.utf8.count), render_ms=\(Int(renderTime*1000)), main_loop_ticks=\(ticks), final section selection/copy")
}

func auditDocumentComments(root: URL, sample: Record) {
    let store = try! Store(root.appendingPathComponent("document-comments.db").path)
    var row = sample
    row.commentsEnabled = true
    row.status = "pending"
    try! store.database.save(row)
    var feedbackPeers: [Int32] = [0, 0]
    precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &feedbackPeers) == 0)
    let status = try! JSONDecoder().decode(Request.self, from: Data("{\"command\":\"status\",\"task_id\":\"\(row.taskID)\",\"sync\":true}".utf8))
    try! store.process(status, Reply(feedbackPeers[0]))
    precondition(store.feedbackWaiters[row.taskID]?.count == 1)
    let commented = try! store.addComment(row.taskID, text: "  Clarify the rollout 🌍  ", quote: "selected paragraph")
    let feedbackHandle = FileHandle(fileDescriptor: feedbackPeers[1], closeOnDealloc: true)
    let immediate = try! JSONSerialization.jsonObject(with: feedbackHandle.readToEnd()!) as! [String: Any]
    precondition(immediate["review_status"] as? String == "open", "Synchronous status returns when comments arrive without closing the reader")
    precondition((immediate["comments"] as? [[String: Any]])?.first?["text"] as? String == "Clarify the rollout 🌍")
    precondition(store.feedbackWaiters[row.taskID] == nil)
    precondition(commented.comments?.first?.text == "Clarify the rollout 🌍")
    precondition(commented.comments?.first?.quote == "selected paragraph")
    precondition(commented.response["review_status"] as? String == "open")
    precondition((commented.response["comments"] as? [[String: Any]])?.count == 1)
    let reopened = try! Database(root.appendingPathComponent("document-comments.db").path)
    precondition((try! reopened.get(row.taskID)).comments?.first?.text == "Clarify the rollout 🌍")
    do { _ = try store.addComment(row.taskID, text: "   ", quote: nil); preconditionFailure("Empty comment accepted") } catch {}
    var peers: [Int32] = [0, 0]
    precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &peers) == 0)
    let wait = try! JSONDecoder().decode(Request.self, from: Data("{\"command\":\"wait\",\"task_id\":\"\(row.taskID)\",\"sync\":false}".utf8))
    try! store.process(wait, Reply(peers[0]))
    precondition(store.waiters[row.taskID]?.count == 1)
    try! store.finish(row.taskID, nil)
    let responseHandle = FileHandle(fileDescriptor: peers[1], closeOnDealloc: true)
    let response = try! JSONSerialization.jsonObject(with: responseHandle.readToEnd()!) as! [String: Any]
    precondition(response["status"] as? String == "ok" && response["review_status"] as? String == "finished")
    precondition((response["comments"] as? [[String: Any]])?.first?["text"] as? String == "Clarify the rollout 🌍")
    do { _ = try store.addComment(row.taskID, text: "late", quote: nil); preconditionFailure("Closed review comment accepted") } catch {}
    var ordinary = row; ordinary.commentsEnabled = false
    precondition(ordinary.response["comments"] == nil && ordinary.response["review_status"] == nil)
    let reader = Preview(commented, openURL: { _ in })
    reader.saveComment = { id, text, quote, commentID, selection, completion in
        do { completion(.success(try store.addComment(id, text: text, quote: quote, commentID: commentID, selection: selection))) } catch { completion(.failure(error)) }
    }
    // Test native sidebar geometry with synthetic content; no production data.
    reader.sidebarCollapsed = false
    waitForNativeReader(reader)
    reader.contentView!.layoutSubtreeIfNeeded()
    reader.reviewSidebar!.arrange()
    precondition(reader.reviewSidebar!.bounds.width == 280)
    precondition(reader.fallbackScroll!.frame.maxX <= reader.reviewSidebar!.frame.minX)
    precondition(reader.reviewSidebar!.entries.subviews.count == 1)
    reader.close()
    print("Passed: durable document comments, synchronous status wakes on first comment while reader stays open, async response payload, synchronous review wait, completion payload, disabled comments omitted, native sidebar geometry")
}

func auditImageNotifications() {
    for (name, size) in [("landscape", NSSize(width: 1000, height: 560)), ("portrait", NSSize(width: 320, height: 960)), ("transparent", NSSize(width: 180, height: 180))] {
        let image = NSImage(size: size, flipped: false) { rect in
            if name != "transparent" { NSColor(calibratedRed: 0.12, green: 0.16, blue: 0.28, alpha: 1).setFill(); rect.fill() }
            NSColor.systemTeal.setFill(); NSBezierPath(roundedRect: rect.insetBy(dx: rect.width * 0.12, dy: rect.height * 0.16), xRadius: 24, yRadius: 24).fill()
            NSColor.systemIndigo.setFill(); NSBezierPath(ovalIn: rect.insetBy(dx: rect.width * 0.28, dy: rect.height * 0.3)).fill()
            return true
        }
        let png = NSBitmapImageRep(data: image.tiffRepresentation!)!.representation(using: .png, properties: [:])!
        var row = Record(taskID: "image-" + name, kind: "update", question: "Updated navigation", project: "Design", title: "Layout preview", description: "Updated navigation", options: [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: 0, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
        row.attachment = DocumentAttachment(name: name + ".png", mime: "image/png", data: png.base64EncodedString())
        row.documentName = name + ".png"
        var opened = 0, completed = 0
        let card = Card(row, open: { opened += 1 }, openURL: { _ in }, complete: { _, _ in completed += 1 })
        card.configure(grouped: true)
        precondition(card.link?.title == "View image")
        let deadline = Date().addingTimeInterval(5)
        while card.imagePreview?.image == nil && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }
        precondition(card.imagePreview?.image != nil)
        precondition(card.imagePreview!.frame.maxY < card.header.frame.minY)
        precondition(card.imagePreview!.frame.minY > card.body.frame.maxY)
        precondition(card.imagePreview!.image!.size.width <= 624 && card.imagePreview!.image!.size.height <= 624)
        card.link!.performClick(nil)
        precondition(opened == 1 && completed == 1)
        let reader = Preview(row, openURL: { _ in })
        while !reader.readerLoaded && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }
        precondition(reader.readerLoaded)
        let picture = reader.text.textStorage!.attribute(.attachment, at: 0, effectiveRange: nil) as! NSTextAttachment
        precondition(picture.image != nil && reader.reviewSidebar == nil)
        if let output = ProcessInfo.processInfo.environment["HEY_BOSS_IMAGE_CAPTURES"] {
            let directory = URL(fileURLWithPath: output)
            try! FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try! png.write(to: directory.appendingPathComponent(name + ".png"))
            let window = NSWindow(contentRect: card.view.bounds.insetBy(dx: -18, dy: -18), styleMask: [.titled], backing: .buffered, defer: false)
            window.contentView = AuditCanvas(frame: window.contentView!.bounds)
            window.contentView!.addSubview(card.view); card.view.setFrameOrigin(NSPoint(x: 18, y: 18))
            for dark in [false, true] {
                window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
                reader.appearance = window.appearance
                for (suffix, view) in [("card", window.contentView!), ("reader", reader.contentView!)] {
                    view.layoutSubtreeIfNeeded()
                    let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
                    view.cacheDisplay(in: view.bounds, to: bitmap)
                    try! bitmap.representation(using: .png, properties: [:])!.write(to: directory.appendingPathComponent("\(name)-\(dark ? "dark" : "light")-\(suffix).png"))
                }
            }
            window.orderOut(nil)
        }
        reader.close()
    }
    print("Passed: image thumbnails, bounded decode, landscape/portrait/transparency, reader and dismiss actions")
}

func auditReviewImage(sample: Record) {
    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 8, pixelsHigh: 8, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    for x in 0..<8 { for y in 0..<8 { bitmap.setColor(.white, atX: x, y: y) } }
    let png = bitmap.representation(using: .png, properties: [:])!
    var row = sample
    row.attachment = DocumentAttachment(name: "sample.png", mime: "image/png", data: png.base64EncodedString())
    row.commentsEnabled = true
    _ = try! row.attachment!.validatedImage()
    let reader = Preview(row, openURL: { _ in })
    let deadline = Date().addingTimeInterval(10)
    while !reader.readerLoaded && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
    precondition(reader.readerLoaded && reader.reviewSidebar != nil)
    let picture = reader.text.textStorage!.attribute(.attachment,at:0,effectiveRange:nil) as! NSTextAttachment
    precondition(picture.image?.size == NSSize(width:8,height:8))
    reader.close()
    let invalid = DocumentAttachment(name: "bad.png", mime: "image/png", data: "not-an-image")
    do { _ = try invalid.validatedImage(); preconditionFailure("Invalid image accepted") } catch {}
    print("Passed: snapshotted image review, native image dimensions and comments sidebar")
}

func auditCommentComposer(root: URL) {
    let store = try! Store(root.appendingPathComponent("composer-review.db").path)
    var row = Record(taskID: "composer-review", kind: "update", question: "# Review\n\nSelected **paragraph**.\nNext line 🌍.\n\n```rust\nlet ready = true; // note\n```", project: "Synthetic", title: "Comment composer", description: "Fixture", options: [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: 0, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
    row.commentsEnabled = true
    try! store.database.save(row)
    let reader = Preview(row, openURL: { _ in })
    reader.saveComment = { id, text, quote, commentID, selection, completion in
        do { completion(.success(try store.addComment(id, text: text, quote: quote, commentID: commentID, selection: selection))) } catch { completion(.failure(error)) }
    }
    reader.finishReview = { id, completion in
        do { try store.finish(id, nil); completion(.success(try store.database.get(id))) } catch { completion(.failure(error)) }
    }
    let deadline = Date().addingTimeInterval(25)
    while !reader.readerLoaded && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
    precondition(reader.readerLoaded)
    let paragraphRange = (reader.text.string as NSString).range(of:"Selected paragraph.\nNext line 🌍.")
    precondition(paragraphRange.location != NSNotFound)
    reader.text.setSelectedRange(paragraphRange)
    let sidebar = reader.reviewSidebar!
    precondition(reader.sidebarCollapsed && sidebar.isHidden)
    reader.inspectSelection()
    let selectionDeadline = Date().addingTimeInterval(5)
    while reader.selectionAction?.isHidden != false && Date() < selectionDeadline { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
    precondition(reader.selectionAction?.isHidden == false)
    reader.selectionAction!.performClick(nil)
    precondition(reader.sidebarCollapsed && sidebar.isHidden)
    precondition(reader.commentEditor?.isVisible == true)
    precondition(sidebar.composerScroll.superview === reader.commentEditor?.contentView)
    precondition(sidebar.composerScroll.superview !== sidebar)
    precondition(sidebar.add.superview == nil && sidebar.finish.superview == nil, "No manual publish or finish controls")
    sidebar.composer.string = "Clarify the selected step."
    reader.textDidChange(Notification(name: NSText.didChangeNotification, object: sidebar.composer))
    while sidebar.comments.isEmpty && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
    precondition(sidebar.comments.first?.text == "Clarify the selected step.")
    precondition(sidebar.comments.first?.quote?.contains("Selected paragraph.") == true && sidebar.comments.first?.quote?.contains("Next line 🌍.") == true)
    precondition(sidebar.comments.first?.selection?.line_start == 3 && sidebar.comments.first?.selection?.line_end == 4)
    precondition(sidebar.comments.first?.selection?.source_text == "Selected **paragraph**.\nNext line 🌍.\n")
    precondition((try! store.database.get(row.taskID)).response["review_status"] as? String == "open", "Comments available before closing")
    sidebar.composer.string = "Clarify the selected step, including timing."
    reader.textDidChange(Notification(name: NSText.didChangeNotification, object: sidebar.composer))
    RunLoop.main.run(until: Date().addingTimeInterval(1.2))
    precondition(sidebar.comments.count == 1 && sidebar.comments[0].text.contains("timing"), "Autosave must update rather than duplicate")
    sidebar.composer.string = "Submitted using the keyboard, including timing."
    reader.textDidChange(Notification(name: NSText.didChangeNotification, object: sidebar.composer))
    let enter = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 0, windowNumber: reader.commentEditor!.windowNumber, context: nil, characters: "\r", charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36)!
    precondition(reader.commentEditor!.performKeyEquivalent(with: enter))
    precondition(reader.commentEditor?.isVisible == false && sidebar.composer.string.isEmpty)
    precondition(sidebar.comments.count == 1 && sidebar.comments[0].text.hasPrefix("Submitted using"))
    reader.commentsToggle!.performClick(nil)
    precondition(!reader.sidebarCollapsed && !sidebar.isHidden)
    sidebar.summaryButtons[0].performClick(nil)
    RunLoop.main.run(until: Date().addingTimeInterval(0.2))
    precondition(reader.commentEditor?.isVisible == true)
    precondition(sidebar.composer.string.contains("timing"))
    reader.commentsToggle!.performClick(nil)
    precondition(reader.sidebarCollapsed && sidebar.isHidden)
    var marks = 0
    reader.text.textStorage!.enumerateAttribute(.reviewComment,in:NSRange(location:0,length:reader.text.textStorage!.length)) { value,_,_ in if value != nil { marks += 1 } }
    precondition(marks > 0,"Saved selection must remain visibly anchored")
    sidebar.composer.string = "Final edit saved on closing."
    reader.textDidChange(Notification(name: NSText.didChangeNotification, object: sidebar.composer))
    reader.performClose(nil)
    RunLoop.main.run(until: Date().addingTimeInterval(1))
    precondition(sidebar.completed && !sidebar.composer.isEditable)
    let closed = try! store.database.get(row.taskID)
    precondition(closed.comments?.count == 1 && closed.comments?.first?.text == "Final edit saved on closing.")
    precondition(closed.response["review_status"] as? String == "finished")
    print("Passed: selection action, floating editor, Cmd+Enter saves/clears/closes, exact multiline Markdown source lines, summary-only collapsible panel, reopen saved comment, debounced automatic publish, immediate status visibility, edits without duplicates, anchored highlights, close flushes and submits")
}

func auditMultilineSourceComment(root: URL) {
    let store = try! Store(root.appendingPathComponent("source-lines.db").path)
    var row = Record(taskID: "source-lines", kind: "update", question: "```rust\nfn main() {\n    let first = true;\n    let next = 2;\n}\n\n```", project: "Synthetic", title: "Source selection", description: "Fixture", options: [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: 0, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
    row.commentsEnabled = true; row.documentName = "fixture.rs"
    try! store.database.save(row)
    let reader = Preview(row, openURL: { _ in })
    var failSave = true
    reader.saveComment = { id, text, quote, commentID, selection, completion in
        if failSave { completion(.failure(StorageError(description: "Synthetic unavailable storage"))); return }
        do { completion(.success(try store.addComment(id, text: text, quote: quote, commentID: commentID, selection: selection))) } catch { completion(.failure(error)) }
    }
    let deadline = Date().addingTimeInterval(25)
    while !reader.readerLoaded && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
    precondition(reader.readerLoaded)
    precondition(reader.sourceSelection(first: 2, last: 2)?.source_text == "fn main() {\n")
    let sourceRange = (reader.text.string as NSString).range(of:"    let first = true;\n    let next = 2;\n")
    precondition(sourceRange.location != NSNotFound)
    reader.text.setSelectedRange(sourceRange)
    reader.inspectSelection()
    while reader.selectionAction?.isHidden != false && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
    precondition(reader.selectedSource?.line_start == 2 && reader.selectedSource?.line_end == 3)
    precondition(reader.selectedSource?.source_text == "    let first = true;\n    let next = 2;\n")
    reader.selectionAction!.performClick(nil)
    reader.reviewSidebar!.composer.string = "Use more descriptive variable names."
    reader.textDidChange(Notification(name: NSText.didChangeNotification, object: reader.reviewSidebar!.composer))
    precondition(reader.commentEditor!.sendButton.isEnabled)
    reader.commentEditor!.sendButton.performClick(nil)
    precondition(reader.commentEditor?.isVisible == true && reader.reviewSidebar!.composer.string == "Use more descriptive variable names.", "Failed save must preserve the draft and keep the editor open")
    precondition((try! store.database.get(row.taskID)).comments == nil)
    failSave = false
    reader.commentEditor!.sendButton.performClick(nil)
    precondition(reader.reviewSidebar!.composer.string.isEmpty && reader.commentEditor?.isVisible == false)
    let saved = try! store.database.get(row.taskID)
    precondition(saved.comments?.first?.selection?.source_text == "    let first = true;\n    let next = 2;\n")
    precondition((saved.response["comments"] as? [[String: Any]])?.first?["selection"] is [String: Any])
    let noNewlineRow = Record(taskID: "no-final-newline", kind: "update", question: "```rust\nlast line\n```", project: "Synthetic", title: "No final newline", description: "Fixture", options: [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: 0, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
    var noNewline = noNewlineRow; noNewline.documentName = "fixture.rs"
    let fallback = Preview(noNewline, openURL: { _ in })
    precondition(fallback.sourceSelection(first: 2, last: 2)?.source_text == "last line", "Do not invent a final LF from the display fence")
    fallback.close()
    reader.close()
    print("Passed: failed save retains draft and editor, retry succeeds, absent final newline preserved, multiline source selection, original source line numbers without fence offset, exact whitespace/newlines in agent metadata, visible Send button saves/clears/closes")
}

// Bind synchronously so the audit does not depend on external interpreter startup.
final class ActionHTTPFixture {
    let port: Int
    private let listener: Int32
    private let stopped = DispatchSemaphore(value: 0)

    init() {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        listener = fd
        precondition(fd >= 0)
        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        precondition(bound == 0 && listen(fd, 8) == 0)
        var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        precondition(withUnsafeMutablePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(fd, $0, &length) }
        } == 0)
        port = Int(UInt16(bigEndian: address.sin_port))
        let completion = stopped
        DispatchQueue(label: "audit.action-http").async {
            defer { completion.signal() }
            while true {
                let client = accept(fd, nil, nil)
                guard client >= 0 else { return }
                Self.respond(client)
                Darwin.close(client)
            }
        }
    }

    func stop() {
        shutdown(listener, SHUT_RDWR)
        Darwin.close(listener)
        precondition(stopped.wait(timeout: .now() + 5) == .success)
    }

    private static func respond(_ client: Int32) {
        var noSignal: Int32 = 1
        setsockopt(client, SOL_SOCKET, SO_NOSIGPIPE, &noSignal, socklen_t(MemoryLayout<Int32>.size))
        var timeout = timeval(tv_sec: 5, tv_usec: 0)
        setsockopt(client, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        var request = Data(), buffer = [UInt8](repeating: 0, count: 4096)
        let separator = Data("\r\n\r\n".utf8)
        while request.count < 262144 {
            let count = recv(client, &buffer, buffer.count, 0)
            guard count > 0 else { return }
            request.append(contentsOf: buffer.prefix(count))
            guard let headerEnd = request.range(of: separator) else { continue }
            let header = String(decoding: request[..<headerEnd.lowerBound], as: UTF8.self)
            let bodyLength = header.components(separatedBy: "\r\n").first {
                $0.lowercased().hasPrefix("content-length:")
            }.flatMap { Int($0.split(separator: ":", maxSplits: 1)[1].trimmingCharacters(in: .whitespaces)) } ?? 0
            guard request.count >= headerEnd.upperBound + bodyLength else { continue }
            let first = header.components(separatedBy: "\r\n")[0].split(separator: " ")
            guard first.count >= 2 else { return }
            var status = "200 OK", extra = ""
            var body = Data("website ready".utf8)
            if first[0] == "POST" {
                status = "201 Created"; extra = "Content-Type: application/json\r\n"
                body = request.subdata(in: headerEnd.upperBound..<(headerEnd.upperBound + bodyLength))
            } else if first[1] == "/large" {
                body = Data(repeating: 120, count: 131073)
            } else if first[1] == "/redirect" {
                status = "302 Found"; extra = "Location: https://example.invalid/blocked\r\n"; body = Data()
            }
            var response = Data("HTTP/1.1 \(status)\r\n\(extra)Content-Length: \(body.count)\r\nConnection: close\r\n\r\n".utf8)
            response.append(body)
            response.withUnsafeBytes { bytes in
                var sent = 0
                while sent < bytes.count {
                    let count = send(client, bytes.baseAddress!.advanced(by: sent), bytes.count - sent, 0)
                    guard count > 0 else { return }
                    sent += count
                }
            }
            return
        }
    }
}

func auditDesktopActions() {
    let server = ActionHTTPFixture()
    defer { server.stop() }
    let port = server.port
    let actions = DesktopActions(cli: nil)
    let pasteboard = NSPasteboard.withUniqueName()
    defer { pasteboard.releaseGlobally() }
    actions.pasteboard = { precondition(Thread.isMainThread); return pasteboard }
    var opened: [URL] = []; var cancelled = 0
    actions.openWebsite = { url, completion in opened.append(url); completion(true) }
    actions.forward = { host, remote in precondition(host == "devbox" && remote == 4123); return port }
    actions.cancel = { host, local, remote in precondition(host == "devbox" && local == port && remote == 4123); cancelled += 1 }
    func invoke(_ id: String, _ method: String, _ params: Any, owner: String = "devbox", version: Int = 1, generation: String = "first") -> [String: Any] {
        let payload = try! JSONSerialization.data(withJSONObject: ["version":version,"id":id,"method":method,"params":params], options: [.sortedKeys])
        let requestData = try! JSONSerialization.data(withJSONObject: ["command":"action","sync":false,"bridge_host":owner,"bridge_generation":generation,"question":String(decoding:payload,as:UTF8.self)])
        let request = try! JSONDecoder().decode(Request.self, from: requestData)
        let lock = NSLock(); var response: [String: Any]?
        actions.handle(request) { value in lock.lock(); response = value; lock.unlock() }
        let deadline = Date().addingTimeInterval(8)
        while Date() < deadline {
            lock.lock(); let result = response; lock.unlock()
            if let result {
                let json = try! JSONSerialization.jsonObject(with: Data((result["result"] as! String).utf8)) as! [String: Any]
                if method.hasPrefix("clipboard.") {
                    actions.queue.sync {
                        let key = owner + ":" + generation + ":" + id
                        precondition(actions.completed[key] == nil && !actions.completionOrder.contains(key), "Clipboard payloads and results must never be cached")
                        precondition(!actions.inFlight.contains(key) && actions.requests[key] == nil)
                    }
                }
                return json
            }
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
        }
        preconditionFailure("Desktop action response timed out")
    }
    let capabilities = invoke("caps", "capabilities", [:])["result"] as! [String: Any]
    precondition((capabilities["methods"] as! [String]).contains("browser.request"))
    precondition((capabilities["methods"] as! [String]).contains("clipboard.copy"))
    precondition((capabilities["methods"] as! [String]).contains("clipboard.paste"))
    precondition(capabilities["clipboard_byte_limit"] as? Int == 128 * 1024)
    let params: [String: Any] = ["url":"http://127.0.0.1:4123/review/token?mode=edit"]
    let first = invoke("open", "browser.open", params)["result"] as! [String: Any]
    let session = first["session"] as! String
    precondition(opened.count == 1 && opened[0].port == port && opened[0].path == "/review/token" && opened[0].query == "mode=edit")
    let duplicate = invoke("open", "browser.open", params)["result"] as! [String: Any]
    precondition(duplicate["session"] as! String == session && opened.count == 1)
    precondition(invoke("open", "browser.open", ["url":"https://example.com"])["error"] != nil)
    let response = invoke("post", "browser.request", ["session":session,"path":"/echo","method":"POST","body":"{\"comment\":\"exact lines 🌍\"}","headers":["Content-Type":"application/json"]])["result"] as! [String: Any]
    precondition(response["status"] as! Int == 201 && response["body"] as! String == "{\"comment\":\"exact lines 🌍\"}")
    precondition(invoke("null-body", "browser.request", ["session":session,"body":NSNull()])["result"] != nil)
    precondition(invoke("bad-body", "browser.request", ["session":session,"body":42])["error"] != nil)
    precondition(invoke("owner", "browser.request", ["session":session], owner:"other.local")["error"] != nil)
    precondition(invoke("escape", "browser.request", ["session":session,"path":"//example.com"])["error"] != nil)
    precondition(invoke("headers", "browser.request", ["session":session,"headers":["Host":"example.com"]])["error"] != nil)
    precondition(invoke("nul", "browser.request", ["session":session,"headers":["X-Header":"bad\u{0}value"]])["error"] != nil)
    precondition(invoke("oversize", "browser.request", ["session":session,"path":"/large"])["error"] != nil)
    let redirect = invoke("redirect", "browser.request", ["session":session,"path":"/redirect"])["result"] as! [String: Any]
    precondition(redirect["status"] as! Int == 302)
    precondition(invoke("future", "capabilities", [:], version:2)["error"] != nil)
    precondition(invoke("unsupported", "system.shell", [:])["error"] != nil)
    precondition(invoke("unsafe", "browser.open", ["url":"file:///tmp/anything"])["error"] != nil)
    precondition(invoke("close", "browser.close", ["session":session])["result"] != nil && cancelled == 1)
    precondition(invoke("closed", "browser.request", ["session":session])["error"] != nil)
    let old = invoke("generation-open", "browser.open", params)["result"] as! [String: Any]
    precondition(invoke("stale", "browser.request", ["session":old["session"]!], generation:"second")["error"] != nil)
    let fresh = invoke("generation-open", "browser.open", params, generation:"second")["result"] as! [String: Any]
    precondition(fresh["session"] as! String != old["session"] as! String)
    precondition(invoke("generation-close", "browser.close", ["session":fresh["session"]!], generation:"second")["result"] != nil)

    let cachedBeforeClipboard = actions.queue.sync { actions.completionOrder }
    func copy(_ text: String) {
        let result = invoke("clipboard-copy", "clipboard.copy", ["base64":Data(text.utf8).base64EncodedString()])["result"] as? [String: Bool]
        precondition(result == ["copied":true])
        precondition(Data(pasteboard.string(forType: .string)!.utf8) == Data(text.utf8), "Copy must preserve exact UTF-8 bytes")
    }
    func paste(_ text: String) {
        let result = invoke("clipboard-paste", "clipboard.paste", [:])["result"] as? [String: String]
        precondition(result == ["base64":Data(text.utf8).base64EncodedString()], "Same-ID paste must read the current clipboard, preserving exact bytes")
    }
    let unicode = "Zażółć — 東京 🌍 e\u{301}\n\r\n\t leading and trailing \n"
    copy(unicode); paste(unicode)
    copy("a\u{0}b\t\r\n"); paste("a\u{0}b\t\r\n")
    pasteboard.clearContents(); precondition(pasteboard.setString("changed externally\n", forType: .string))
    paste("changed externally\n")
    // An explicit second copy with the same ID must execute, not replay an old response.
    copy(unicode); paste(unicode)
    copy(""); paste("")
    pasteboard.clearContents(); paste("")
    precondition(pasteboard.setData(Data([0, 1, 2]), forType: .init("org.hey-boss.audit.binary")))
    paste("")

    let limit = 128 * 1024
    let boundary = String(repeating: "🌍", count: limit / 4)
    precondition(boundary.utf8.count == limit)
    copy(String(repeating: "a", count: limit)); paste(String(repeating: "a", count: limit))
    copy(boundary); paste(boundary)
    let marker = Data([3, 2, 1]), markerType = NSPasteboard.PasteboardType("org.hey-boss.audit.marker")
    precondition(pasteboard.setData(marker, forType: markerType))
    let unchanged = pasteboard.changeCount
    func rejected(_ method: String, _ params: Any) {
        let response = invoke("clipboard-rejected", method, params)
        precondition(response["error"] != nil && response["result"] == nil)
        precondition(pasteboard.changeCount == unchanged, "Invalid input must not clear or change the clipboard")
        precondition(Data(pasteboard.string(forType: .string)!.utf8) == Data(boundary.utf8))
        precondition(pasteboard.data(forType: markerType) == marker)
    }
    for invalid in ["%%%", "YQ", "YQ=", "YQ===", "YQ==\n", "Y Q==", "YQ==YQ==", "YR==", "____"] {
        rejected("clipboard.copy", ["base64":invalid])
    }
    for invalid: [UInt8] in [[0xff], [0xc0, 0xaf], [0xe2, 0x82], [0xed, 0xa0, 0x80]] {
        rejected("clipboard.copy", ["base64":Data(invalid).base64EncodedString()])
    }
    rejected("clipboard.copy", ["base64":Data((boundary + "a").utf8).base64EncodedString()])
    rejected("clipboard.copy", ["base64":Data(String(repeating: "a", count: limit + 1).utf8).base64EncodedString()])
    for malformed: [String: Any] in [[:], ["text":"do not copy"], ["base64":42], ["base64":true], ["base64":NSNull()], ["base64":["YQ=="]], ["base64":"", "extra":true]] {
        rejected("clipboard.copy", malformed)
    }
    for malformed: [String: Any] in [["base64":""], ["extra":true]] { rejected("clipboard.paste", malformed) }
    for malformed: Any in [NSNull(), "text", 42, true, [] as [Any]] {
        rejected("clipboard.copy", malformed); rejected("clipboard.paste", malformed)
    }
    // Oversize native text is rejected on paste without changing it or retaining the result.
    pasteboard.clearContents(); precondition(pasteboard.setString(boundary + "a", forType: .string))
    let oversizeChangeCount = pasteboard.changeCount
    precondition(invoke("clipboard-paste", "clipboard.paste", [:])["error"] != nil)
    precondition(pasteboard.changeCount == oversizeChangeCount && Data(pasteboard.string(forType: .string)!.utf8) == Data((boundary + "a").utf8))
    copy("after error"); paste("after error")
    actions.queue.sync {
        precondition(actions.completionOrder == cachedBeforeClipboard)
        precondition(Set(actions.completed.keys) == Set(cachedBeforeClipboard), "Clipboard actions must leave browser caches intact")
    }
    print("Passed: private main-thread clipboard, exact Unicode/newlines/empty text, non-text paste, 128 KiB boundary, invalid base64/UTF-8/params preserve clipboard, uncached same-ID copy/paste, browser cache preservation")
    print("Passed: connection generation isolation, desktop capabilities, remote browser forward, exact path/query, idempotent open, conflicting IDs, Unicode POST round trip, owner/origin isolation, unsafe headers, bounded responses, no cross-origin redirects, unsupported versions/actions, session release")
}

func auditPerformance() {
    let overview = AgentsOverview(present: false, cli: nil, preferences: nil)
    let source = makeAgentPreview().local!.agents[0]
    let now = Date().timeIntervalSince1970
    var agents: [AgentInfo] = []
    for index in 0..<1000 {
        let repo = index % 50
        let git = AgentGit(repositoryRoot: "/work/project-\(repo)", commonDir: "/work/project-\(repo)/.git", worktree: "/work/project-\(repo)", branch: "feature-\(index)", repositoryId: "example.com/project-\(repo)", origin: "example.com/project-\(repo)")
        agents.append(AgentInfo(id: "perf-\(index)", pid: UInt32(100 + index), kind: "Codex", cwd: git.worktree, sessionId: "session-\(index)", task: source.task, activity: source.activity, state: index % 3 == 0 ? "Working" : "Idle", updatedAt: now, evidence: source.evidence, update: source.update, activityAt: now, git: git))
    }
    overview.local = AgentSnapshot(host: "benchmark", observedAt: now, agents: agents, warnings: [])
    var warm: [Double] = [], search: [Double] = []
    for _ in 0..<30 {
        overview.search.stringValue = ""; overview.rebuild(); warm.append(overview.performanceMetrics()["rebuild_ms"]!)
        overview.search.stringValue = "feature-1"; overview.rebuild(); search.append(overview.performanceMetrics()["rebuild_ms"]!)
    }
    func stats(_ samples: [Double]) -> [String: Double] { let sorted = samples.sorted(); return ["median": sorted[sorted.count / 2], "p95": sorted[Int(Double(sorted.count - 1) * 0.95)], "max": sorted.last!] }
    overview.search.stringValue = ""; overview.rebuild()
    var click: [Double] = [], draw: [Double] = []
    for _ in 0..<30 {
        let button = NSButton(); button.tag = 0
        let start = ProcessInfo.processInfo.systemUptime
        overview.toggleGroup(button)
        click.append((ProcessInfo.processInfo.systemUptime - start) * 1000)
        let frameStart = ProcessInfo.processInfo.systemUptime
        overview.window.contentView?.layoutSubtreeIfNeeded()
        overview.window.displayIfNeeded()
        draw.append((ProcessInfo.processInfo.systemUptime - frameStart) * 1000)
    }
    let metrics: [String: Any] = ["click_handler_ms": stats(click), "layout_display_ms": stats(draw), "interaction_metrics": overview.performanceMetrics(), "agents": agents.count, "warm_rebuild_ms": stats(warm), "search_ms": stats(search)]
    print(String(decoding: try! JSONSerialization.data(withJSONObject: metrics, options: [.sortedKeys]), as: UTF8.self))
}

final class HealthAuditTextView: NSTextView {
    var copiedText = ""
    override func copy(_ sender: Any?) { copiedText = (string as NSString).substring(with: selectedRange()) }
    override func paste(_ sender: Any?) { insertText("pasted", replacementRange: selectedRange()) }
}

func auditSecretInput() {
    let long = String(repeating: "synthetic-key-", count: 2000)
    let editor = GrowingTextInput(frame: NSRect(x: 0, y: 0, width: 432, height: 64))
    var wanted: CGFloat = 0; editor.heightChanged = { wanted = $0 }
    editor.stringValue = long
    precondition(editor.stringValue == long && wanted == 220 && editor.text.frame.height > 220)
    precondition(editor.text.isHorizontallyResizable == false && editor.text.isAutomaticQuoteSubstitutionEnabled == false)
    let requestData = try! JSONSerialization.data(withJSONObject: ["command":"secret", "sync":true, "project":"Synthetic test", "title":"Credentials", "question":"{\"fields\":[\"LOGIN\",\"PASSWORD\"],\"login\":true,\"destination\":\"Synthetic private file\"}"])
    let request = try! JSONDecoder().decode(Request.self, from: requestData)
    var form = SecretForm.decode(request)!
    let legacy = SecretPrompt(request: request, form: form) { _ in }
    precondition(legacy.mode.isHidden, "Old callers cannot receive public replies")
    form.reply = true
    var result: SecretOutcome?
    let prompt = SecretPrompt(request: request, form: form) { result = $0 }
    func capture(_ prompt: SecretPrompt, _ name: String) {
        guard let path = ProcessInfo.processInfo.environment["HEY_BOSS_SECRET_SCREENSHOT"] else { return }
        // Render this in-memory synthetic view only, never the desktop or an
        // installed credential prompt. Print rendering omits compositor-hosted
        // controls; review those interactively when screen capture is available.
        prompt.window.contentView?.layoutSubtreeIfNeeded()
        let view = prompt.window.contentView!
        precondition(view.bounds.contains(prompt.submit.frame), "Submit must remain visible at every size")
        let base = URL(fileURLWithPath: path)
        let url = name.isEmpty ? base : base.deletingPathExtension().appendingPathExtension(name + ".png")
        prompt.window.effectiveAppearance.performAsCurrentDrawingAppearance {
            let pdf = NSImage(data: view.dataWithPDF(inside: view.bounds))!
            let rendered = NSImage(size: view.bounds.size)
            rendered.lockFocus()
            NSColor.windowBackgroundColor.setFill(); view.bounds.fill()
            pdf.draw(in: view.bounds)
            rendered.unlockFocus()
            let bitmap = NSBitmapImageRep(data: rendered.tiffRepresentation!)!
            try! bitmap.representation(using: .png, properties: [:])!.write(to: url)
        }
    }
    precondition(!prompt.entries[0].maskedMode && prompt.entries[1].maskedMode && prompt.window.sharingType == .none)
    precondition(!prompt.submit.isEnabled)
    prompt.window.appearance = NSAppearance(named: .aqua)
    capture(prompt, "empty")
    prompt.entries[0].revealed.stringValue = "synthetic-login"
    prompt.entries[1].masked.stringValue = long
    prompt.entries.forEach { $0.updateCount() }; precondition(prompt.submit.isEnabled)
    prompt.entries[1].toggle.state = .on; prompt.entries[1].toggleVisibility()
    precondition(prompt.entries[1].revealed.stringValue == long && prompt.entries[1].masked.stringValue.isEmpty)
    prompt.window.contentView?.layoutSubtreeIfNeeded()
    capture(prompt, "revealed")
    prompt.window.setContentSize(NSSize(width: 600, height: 420))
    capture(prompt, "compact-revealed")
    prompt.entries[1].toggle.state = .off; prompt.entries[1].toggleVisibility()
    precondition(prompt.entries[1].masked.stringValue == long && prompt.entries[1].revealed.stringValue.isEmpty)
    capture(prompt, "compact-masked")
    prompt.window.setContentSize(NSSize(width: 680, height: 560))
    capture(prompt, "")
    prompt.window.appearance = NSAppearance(named: .darkAqua)
    capture(prompt, "dark")
    prompt.entries[1].masked.stringValue = String(repeating: "x", count: 65537)
    prompt.entries[1].updateCount(); precondition(!prompt.submit.isEnabled)
    capture(prompt, "invalid")
    prompt.finish(); precondition(result == nil, "Invalid input must not deliver")
    prompt.entries[1].masked.stringValue = long; prompt.entries[1].updateCount()
    prompt.finish(); precondition(result == .submitted(["synthetic-login",long]))
    precondition(prompt.entries.allSatisfy { $0.value.isEmpty } && prompt.completion == nil)
    let singleForm = SecretForm(fields: ["API_KEY"], login: false, destination: "Synthetic private file")
    let single = SecretPrompt(request: request, form: singleForm) { result = $0 }
    single.window.appearance = NSAppearance(named: .aqua)
    precondition(single.entries[0].maskedMode && !single.submit.isEnabled)
    capture(single, "single-empty")
    single.entries[0].masked.stringValue = long; single.entries[0].updateCount()
    precondition(single.submit.isEnabled)
    capture(single, "single-masked")
    single.entries[0].toggle.state = .on; single.entries[0].toggleVisibility()
    capture(single, "single-revealed")
    single.cancel(); precondition(single.entries[0].value.isEmpty && result == .cancelled)
    let responseText = "Synthetic reply\nUse the existing configuration 🦀."
    let responsePrompt = SecretPrompt(request: request, form: form) { result = $0 }
    responsePrompt.entries[1].masked.stringValue = long
    responsePrompt.mode.selectedSegment = 1; responsePrompt.changeMode()
    precondition(responsePrompt.credentialContent.isHidden && !responsePrompt.replyContent.isHidden)
    precondition(!responsePrompt.submit.isEnabled && responsePrompt.submit.title == "Send reply")
    responsePrompt.window.appearance = NSAppearance(named: .aqua)
    capture(responsePrompt, "reply-empty")
    for invalid in [" \n\t", String(repeating: "x", count: 8193), "synthetic\u{0}", "synthetic\u{1b}"] {
        responsePrompt.replyInput.stringValue = invalid; responsePrompt.validate()
        precondition(!responsePrompt.submit.isEnabled)
    }
    capture(responsePrompt, "reply-invalid")
    responsePrompt.replyInput.stringValue = responseText; responsePrompt.validate()
    precondition(responsePrompt.submit.isEnabled)
    capture(responsePrompt, "reply-light")
    responsePrompt.window.appearance = NSAppearance(named: .darkAqua)
    capture(responsePrompt, "reply-dark")
    responsePrompt.window.setContentSize(NSSize(width: 600, height: 398))
    capture(responsePrompt, "reply-compact")
    responsePrompt.replyInput.stringValue = String(repeating: "Synthetic long reply line.\n", count: 100)
    responsePrompt.validate(); capture(responsePrompt, "reply-long")
    responsePrompt.mode.selectedSegment = 0; responsePrompt.changeMode()
    precondition(responsePrompt.replyContent.isHidden && responsePrompt.submit.title == "Use credentials")
    precondition(!responsePrompt.submit.isEnabled, "Reply text must never satisfy credential validation")
    responsePrompt.mode.selectedSegment = 1; responsePrompt.changeMode()
    responsePrompt.replyInput.stringValue = responseText; responsePrompt.finish()
    precondition(result == .replied(responseText))
    precondition(responsePrompt.entries.allSatisfy { $0.value.isEmpty } && responsePrompt.replyInput.stringValue.isEmpty)
    responsePrompt.cancel(); precondition(result == .replied(responseText), "Completion is exactly once")
    var cancelled = false
    let discard = SecretPrompt(request: request, form: form) { cancelled = $0 == .cancelled }
    discard.entries[1].masked.stringValue = long; discard.replyInput.stringValue = responseText; discard.cancel()
    precondition(cancelled && discard.entries.allSatisfy { $0.value.isEmpty } && discard.replyInput.stringValue.isEmpty)
    var expired = false
    let timeout = SecretPrompt(request: request, form: form) { expired = $0 == .expired }
    timeout.entries[1].masked.stringValue = long; timeout.replyInput.stringValue = responseText; timeout.complete(.expired)
    precondition(expired && timeout.entries.allSatisfy { $0.value.isEmpty } && timeout.replyInput.stringValue.isEmpty)
    let prompts = SecretPrompts(present: false)
    var peers: [Int32] = [0,0]; precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &peers) == 0)
    let reply = Reply(peers[1]); shutdown(peers[0], SHUT_WR)
    precondition(reply.isConnected)
    prompts.handle(request, reply)
    let active = prompts.active.values.first!
    active.entries[0].revealed.stringValue = "synthetic-login"
    active.entries[1].masked.stringValue = long
    active.finish()
    let received = FileHandle(fileDescriptor: peers[0], closeOnDealloc: true)
    let response = try! JSONSerialization.jsonObject(with: received.readDataToEndOfFile()) as! [String: Any]
    precondition(response["status"] as? String == "ok" && prompts.active.isEmpty)
    try! received.close()
    precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &peers) == 0)
    signal(SIGPIPE, SIG_IGN)
    let disconnected = Reply(peers[1]); shutdown(peers[0], SHUT_WR)
    prompts.handle(request, disconnected); let pending = prompts.active.values.first!
    pending.entries[1].masked.stringValue = long
    Darwin.close(peers[0])
    // Wait for the polling callback even when the host is busy compiling.
    // Completion still requires both removal and clearing the synthetic values.
    let disconnectDeadline = Date().addingTimeInterval(10)
    while !prompts.active.isEmpty && Date() < disconnectDeadline {
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    }
    precondition(prompts.active.isEmpty && pending.entries.allSatisfy { $0.value.isEmpty }, "Disconnected secret requests must be removed and their fields cleared")
    for expected in ["cancelled", "expired", "rejected", "busy", "replied"] {
        precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &peers) == 0)
        let statusReply = Reply(peers[1]); shutdown(peers[0], SHUT_WR)
        if expected == "rejected" {
            var raw = try! JSONSerialization.jsonObject(with: requestData) as! [String: Any]
            raw["sync"] = false
            let invalid = try! JSONDecoder().decode(Request.self, from: JSONSerialization.data(withJSONObject: raw))
            prompts.handle(invalid, statusReply)
        } else if expected == "busy" {
            // In-memory fixtures only; no extra requests or real secret windows.
            for _ in 0..<4 { prompts.active[UUID()] = SecretPrompt(request: request, form: form) { _ in } }
            prompts.handle(request, statusReply)
            prompts.active.removeAll()
        } else {
            prompts.handle(request, statusReply)
            prompts.active.values.first!.complete(expected == "replied" ? .replied(responseText) : expected == "expired" ? .expired : .cancelled)
        }
        let reader = FileHandle(fileDescriptor: peers[0], closeOnDealloc: true)
        let response = try! JSONSerialization.jsonObject(with: reader.readDataToEndOfFile()) as! [String: Any]
        precondition(response["status"] as? String == expected && response["task_id"] as? String == "secret")
        precondition(prompts.active.isEmpty)
        if expected == "replied" { precondition(response["result"] as? String == responseText) }
        else { precondition(response["result"] == nil) }
        try! reader.close()
    }
    let ui = Interface(present: false)
    let item = Record(taskID: UUID().uuidString, kind: "prompt", question: "Enter a long answer", project: "Synthetic test", title: "Long text", description: "", options: [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: Date().timeIntervalSince1970, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
    ui.add(item); let oldHeight = ui.question.frame.height
    ui.field!.stringValue = long
    precondition(ui.field!.stringValue == long && ui.field!.frame.height == 220 && ui.question.frame.height > oldHeight)
    ui.question.orderOut(nil)
    print("Passed: long wrapping input, masked credential pair, reveal, cancellation and clearing")
}


func auditWebInbox(root:URL) {
    let store=try! Store(root.appendingPathComponent("web-inbox.db").path)
    func request(_ command:String,_ id:String?=nil,_ fields:[String:Any]=[:])->[String:Any] {
        var raw=fields;raw["command"]=command;raw["sync"]=false;if let id { raw["task_id"]=id }
        let decoded=try! JSONDecoder().decode(Request.self,from:JSONSerialization.data(withJSONObject:raw))
        var fds:[Int32]=[0,0];precondition(socketpair(AF_UNIX,SOCK_STREAM,0,&fds)==0)
        store.handle(decoded,Reply(fds[1]))
        let input=FileHandle(fileDescriptor:fds[0],closeOnDealloc:true)
        let response=try! JSONSerialization.jsonObject(with:input.readDataToEndOfFile()) as! [String:Any]
        if response["status"] as? String == "ok" { precondition(!(response["task_id"] as? String ?? "").isEmpty, "Inbox replies must support the CLI response contract") }
        if let result=response["result"] as? String { return try! JSONSerialization.jsonObject(with:Data(result.utf8)) as! [String:Any] }
        return response
    }
    let creation=request("update",nil,["project":"Fixture","title":"Report","question":"# Markdown","description":"Summary","issue":["project":"github.com/example/repo","number":7],"comments_enabled":false])
    let id=creation["task_id"] as! String
    let reference=try! store.database.get(id).issue
    precondition(reference?.number==7)
    let listReply=request("inbox_list")
    let summaries=listReply["tasks"] as! [[String:Any]]
    precondition(summaries.count==1 && summaries[0]["question"]==nil && summaries[0]["attachment"]==nil)
    let countReply=request("inbox_list",nil,["count_only":true])
    precondition(countReply["unread"] as? Int==1 && countReply["tasks"]==nil, "Unread badges must not receive notice rows")
    let historical = Record(taskID:"historical",kind:"update",question:String(repeating:"Archived document body.",count:10000),project:"History only",title:"Old report",description:"",options:[],autoclose:nil,linkURL:nil,linkLabel:nil,createdAt:0,presentedAt:nil,expiresAt:nil,status:"ok",result:nil,origin:nil)
    try! store.database.save(historical)
    try! store.database.execute("INSERT INTO dialogs(id,status,body) VALUES('damaged','ok','invalid json')")
    let unreadReply = request("inbox_unread")
    precondition((unreadReply["tasks"] as! [[String:Any]]).count == 1)
    precondition(unreadReply["unread"] as? Int == 1 && unreadReply["archived"] as? Int == 1)
    precondition((unreadReply["projects"] as! [String]).contains("History only"))
    precondition((try! JSONSerialization.data(withJSONObject:unreadReply)).count < 4096, "Unread must not transfer archived documents")
    try! store.database.execute("DELETE FROM dialogs WHERE id IN ('historical','damaged')")
    precondition((request("inbox_view",id)["task"] as! [String:Any])["status"] as? String=="pending")
    _=request("inbox_link",id,["issue":["project":"github.com/example/repo","number":8]])
    var row=try! store.database.get(id)
    precondition(row.status=="pending" && row.issue?.number==8 && row.question=="# Markdown" && row.completedAt==nil)
    _=request("inbox_link",id);row=try! store.database.get(id);precondition(row.issue==nil && row.status=="pending")
    _=request("inbox_read",id);row=try! store.database.get(id);precondition(row.status=="ok" && row.completedAt != nil)
    _=request("inbox_link",id,["issue":["project":"github.com/example/repo","number":9]])
    precondition((try! store.database.get(id)).status=="ok")
    var question=Record(taskID:"question",kind:"approval",question:"Ship?",project:"Fixture",title:"Ship",description:"Details",options:["Approve","Reject"],autoclose:nil,linkURL:nil,linkLabel:nil,createdAt:1,presentedAt:nil,expiresAt:nil,status:"pending",result:nil,origin:nil)
    try! store.database.save(question)
    _=request("inbox_read","question");precondition((try! store.database.get("question")).status=="pending")
    precondition(request("inbox_respond","question",["question":"Invalid"])["status"] as? String=="error")
    _=request("inbox_respond","question",["question":"Approve"])
    _=request("inbox_respond","question",["question":"Reject"])
    precondition((try! store.database.get("question")).result=="Approve")
    question=Record(taskID:"cancel",kind:"prompt",question:"Why?",project:"Fixture",title:"Why",description:"",options:[],autoclose:nil,linkURL:nil,linkLabel:nil,createdAt:1,presentedAt:nil,expiresAt:nil,status:"pending",result:nil,origin:nil)
    try! store.database.save(question);_=request("inbox_dismiss","cancel")
    precondition((try! store.database.get("cancel")).status=="cancelled" && (try! store.database.get("cancel")).result==nil)
    var review=Record(taskID:"review",kind:"update",question:"# Review",project:"Fixture",title:"Review",description:"",options:[],autoclose:nil,linkURL:nil,linkLabel:nil,createdAt:1,presentedAt:nil,expiresAt:nil,status:"pending",result:nil,origin:nil)
    review.commentsEnabled=true;try! store.database.save(review);_=request("inbox_read","review")
    precondition((try! store.database.get("review")).status=="pending")
    _=request("inbox_comment","review",["question":"**Feedback**","description":"Quoted source"])
    precondition((try! store.database.get("review")).comments?.first?.quote=="Quoted source")
    _=request("inbox_finish_review","review")
    precondition((try! store.database.get("review")).status=="ok")
    precondition(request("inbox_link",id,["issue":["project":"","number":0]])["status"] as? String=="error")
    for (taskID,kind,isReview) in [("clear-update","update",false),("clear-alert","alert",false),("clear-question","approval",false),("clear-prompt","prompt",false),("clear-review","update",true),("new-arrival","alert",false)] {
        var item=Record(taskID:taskID,kind:kind,question:"Body",project:"Fixture",title:taskID,description:"Summary",options:["Approve","Reject"],autoclose:nil,linkURL:nil,linkLabel:nil,createdAt:1,presentedAt:nil,expiresAt:nil,status:"pending",result:nil,origin:nil)
        item.commentsEnabled=isReview
        try! store.database.save(item)
    }
    precondition(request("inbox_clear",nil,["task_ids":["clear-update","missing"]])["status"] as? String=="error")
    precondition((try! store.database.get("clear-update")).status=="pending")
    for ids in [[],[""],["clear-update","clear-update"]] as [[String]] { precondition(request("inbox_clear",nil,["task_ids":ids])["status"] as? String=="error") }
    let winnerEncoder=JSONEncoder();winnerEncoder.outputFormatting = [.sortedKeys]
    let winners=try! winnerEncoder.encode(store.database.get("question"))
    let clearedIDs=["clear-update","clear-alert","clear-question","clear-prompt","clear-review","question",id]
    precondition(request("inbox_clear",nil,["task_ids":clearedIDs])["cleared"] as? Int==5)
    for taskID in clearedIDs.prefix(5) {
        let item=try! store.database.get(taskID)
        precondition(item.status == (taskID=="clear-update" || taskID=="clear-alert" ? "ok" : "cancelled") && item.result==nil && item.completedAt != nil)
    }
    precondition((try! store.database.get("new-arrival")).status=="pending")
    precondition(try! winnerEncoder.encode(store.database.get("question"))==winners)
    precondition(request("inbox_clear",nil,["task_ids":clearedIDs])["cleared"] as? Int==0)
    precondition((try! store.database.pendingCount())==1)
    try! store.database.execute("INSERT INTO dialogs VALUES('damaged','pending','invalid JSON')")
    let finalCount=request("inbox_list",nil,["count_only":true])
    let finalList=request("inbox_list")
    precondition(finalCount["unread"] as? Int==finalList["unread"] as? Int && finalCount["unread"] as? Int==1)
    precondition(finalCount["tasks"]==nil && (try! JSONSerialization.data(withJSONObject:finalCount)).count<100)
    let linked=request("inbox_list",nil,["issue":["project":"github.com/example/repo","number":9]])
    let linkedRows=linked["tasks"] as! [[String:Any]]
    precondition(linkedRows.count==1 && linkedRows[0]["taskID"] as? String==id, "Related notices must exclude other issues and unlinked history")
    precondition(linked["unread"] as? Int==finalCount["unread"] as? Int, "Related reads must keep the global unread total")
    for reference in [["project":"github.com/example/repo","number":8],["project":"github.com/other/repo","number":9],["project":"github.com/example/repo","number":9,"host":"remote"]] as [[String:Any]] {
        precondition((request("inbox_list",nil,["issue":reference])["tasks"] as! [[String:Any]]).isEmpty)
    }
    precondition(request("inbox_list",nil,["issue":["project":"","number":0]])["status"] as? String=="error")
    let relatedReference=IssueReference(project:"github.com/example/repo",number:9,host:nil)
    func relatedReadSteps() -> Int {
        var steps=0
        withUnsafeMutablePointer(to:&steps) { pointer in
            sqlite3_trace_v2(store.database.db,UInt32(SQLITE_TRACE_PROFILE),{ _,context,statement,_ in
                if let context, let statement {
                    context.assumingMemoryBound(to:Int.self).pointee += Int(sqlite3_stmt_status(OpaquePointer(statement),SQLITE_STMTSTATUS_VM_STEP,0))
                }
                return 0
            },pointer)
            defer { sqlite3_trace_v2(store.database.db,0,nil,nil) }
            precondition((try! store.database.inboxRelatedRows(relatedReference)).count==1)
            precondition((try! store.database.inboxUnreadCount())==1)
        }
        return steps
    }
    let stepsBefore=relatedReadSteps()
    try! store.database.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<8192) INSERT INTO dialogs SELECT 'history-'||x,'ok',json_object('kind','update','title','Unrelated history','issue',json_object('project','other','number',9)) FROM n")
    let stepsAfter=relatedReadSteps()
    precondition(stepsAfter <= stepsBefore+20, "Related reads must not scan unrelated history")
    let scaledUnread = request("inbox_unread")
    precondition((scaledUnread["tasks"] as! [[String:Any]]).count == 1)
    precondition((scaledUnread["archived"] as! Int) >= 8192)
    precondition((try! JSONSerialization.data(withJSONObject:scaledUnread)).count < 4096, "Unread transfer must stay bounded as Activity grows")
    print("Related notice lookup: \(stepsBefore) → \(stepsAfter) VM steps after 8192 unrelated records")
    _=request("inbox_link",id,["issue":["project":"github.com/example/repo","number":9,"host":"remote"]])
    precondition((try! store.database.inboxRelatedRows(relatedReference)).isEmpty)
    precondition((request("inbox_list",nil,["issue":["project":"github.com/example/repo","number":9,"host":"remote"]])["tasks"] as! [[String:Any]]).count==1)
    print("Passed: web Inbox summaries, Markdown, creation links, relationship-only link/unlink, archived linking, read receipts, question read safety, invalid answers, winning answer preservation, cancellation, review comments and finish")
}

func auditOptimisticNotifications() {
    let ui = Interface(present: false)
    func row(_ id: String, kind: String = "alert") -> Record {
        Record(taskID: id, kind: kind, question: "Notification content", project: "Synthetic", title: "Open me", description: "Description", options: [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: 1, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
    }
    let notice = row("notice")
    var sent: [String] = []
    ui.onComplete = { id, _ in sent.append(id) } // Deliberately never acknowledges.
    ui.onDismissMany = { sent += $0 }
    ui.add(notice)
    ui.cards[0].contentClick.activate()
    precondition(ui.previews[notice.taskID] != nil, "Clicking the card must open it")
    precondition(ui.cards.isEmpty && sent == [notice.taskID], "Opening must dismiss immediately without an acknowledgement")
    ui.add(notice)
    precondition(ui.cards.isEmpty, "A stale refresh cannot resurrect an in-flight dismissal")
    ui.retryCompletion([notice.taskID], message: "Synthetic offline response")
    precondition(ui.cards.count == 1, "Failed actions must restore a retryable notice")
    ui.add(row("second")); ui.closeAll.invoke()
    precondition(ui.cards.isEmpty, "Close all must not wait for requests")
    ui.add(row("arrival"))
    ui.remove(notice.taskID) // First acknowledgement succeeded; second failed.
    ui.retryCompletion([notice.taskID, "second"], message: "Synthetic partial failure")
    precondition(Set(ui.cards.map { $0.row.taskID }) == Set(["second", "arrival"]), "Restore only failed notices and preserve concurrent arrivals")
    ui.add(row("question", kind: "prompt"))
    ui.field!.stringValue = "Keep this draft"
    ui.answer("Keep this draft")
    precondition(ui.current == nil, "Submitting moves on before acknowledgement")
    ui.retryCompletion(["question"], message: "Synthetic offline response")
    precondition(ui.current?.taskID == "question" && ui.field?.stringValue == "Keep this draft", "Restore unsent answer text")
    Array(ui.previews.values).forEach { $0.close() }
    var review = row("review-draft",kind:"update")
    review.commentsEnabled = true
    let preview = Preview(review,openURL:{ _ in })
    preview.reviewSidebar!.composer.string = "Retain unsaved review feedback"
    var save: ((Result<Record,Error>) -> Void)?
    preview.saveComment = { _,_,_,_,_,completion in save = completion }
    preview.makeKeyAndOrderFront(nil)
    preview.performClose(nil)
    precondition(!preview.isVisible && save != nil, "Review close waits for draft persistence")
    save?(.failure(StorageError(description:"Synthetic draft save failure")))
    precondition(preview.isVisible && preview.reviewSidebar!.composer.string == "Retain unsaved review feedback", "Failed review save loses the draft")
    preview.close()
    let large = Record(taskID:"large",kind:"update",question:String(repeating:"# A heading\n\nSome **formatted** content with a [link](https://example.com).\n\n",count:15000),project:"Synthetic",title:"Large document",description:"",options:[],autoclose:nil,linkURL:nil,linkLabel:nil,createdAt:1,presentedAt:nil,expiresAt:nil,status:"pending",result:nil,origin:nil)
    let started = ProcessInfo.processInfo.systemUptime
    let largePreview = Preview(large,openURL:{ _ in })
    let elapsed = ProcessInfo.processInfo.systemUptime - started
    precondition(elapsed < 0.5, "Large document opening parses the entire body on the main thread")
    precondition(largePreview.text.string.isEmpty && largePreview.loadingView != nil)
    largePreview.close()
    print("Large document initial preview: \(Int(elapsed * 1000))ms")
    print("Passed: card clicks open immediately; optimistic read, dismiss-all and answers; stale-refresh suppression; partial rollback and draft recovery")
}


func auditNotificationClicks() {
    let window = panel("Synthetic notification click audit")
    window.setFrame(NSRect(x:80,y:100,width:360,height:250),display:true)
    var opens = 0, completions = 0, toggles = 0
    let row = Record(taskID:"click",kind:"update",question:"# Document",project:"Clicks",title:"Clickable title",description:"Clickable notification content",options:[],autoclose:nil,linkURL:nil,linkLabel:nil,createdAt:1,presentedAt:nil,expiresAt:nil,status:"pending",result:nil,origin:nil)
    let card = Card(row,open:{ opens += 1 },openURL:{ _ in },complete:{ _,_ in completions += 1 })
    window.contentView!.addSubview(card.view)
    window.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps:true)
    RunLoop.main.run(until:Date().addingTimeInterval(0.2))
    func snapshot(_ expanded: Bool?) {
        guard let directory = ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_SNAPSHOT_DIR"] else { return }
        try! FileManager.default.createDirectory(atPath:directory,withIntermediateDirectories:true)
        let state = expanded.map { $0 ? "expanded" : "collapsed" } ?? "card"
        for dark in [false, true] {
            // Bitmap exports need solid backing for compositor-owned glass.
            // Use duplicate views so captures cannot alter the event fixture.
            let sample = Card(row,open:{},openURL:{ _ in },complete:{ _,_ in })
            sample.configure(grouped:expanded != nil)
            sample.view.drawsSurface = false
            let group = ProjectGroup("Clicks",toggle:{},clear:{})
            if let expanded { group.update([sample],expanded:expanded) }
            let surface = expanded == nil ? sample.view : group.view
            surface.drawsSurface = false
            let canvas = AuditCanvas(frame:NSRect(x:0,y:0,width:360,height:surface.frame.height + 16))
            surface.frame.origin = NSPoint(x:8,y:8)
            canvas.addSubview(surface)
            let preview = NSWindow(contentRect:canvas.bounds,styleMask:[.borderless],backing:.buffered,defer:false)
            preview.isReleasedWhenClosed = false
            preview.contentView = canvas
            preview.appearance = NSAppearance(named:dark ? .darkAqua : .aqua)
            canvas.layoutSubtreeIfNeeded()
            let bitmap = canvas.bitmapImageRepForCachingDisplay(in:canvas.bounds)!
            canvas.cacheDisplay(in:canvas.bounds,to:bitmap)
            let path = URL(fileURLWithPath:directory).appendingPathComponent("notification-click-\(state)-\(dark ? "dark" : "light").png")
            try! bitmap.representation(using:.png,properties:[:])!.write(to:path)
            withExtendedLifetime((sample,group)) { preview.close() }
        }
    }
    func click(_ view: NSView, drag: Bool = false) {
        // The audit drives AppKit without NSApplication.run(). Earlier fixtures
        // leave window/activation events queued; dispatch those before timestamping
        // this gesture, just as an idle application's event loop would.
        let readyBy = ProcessInfo.processInfo.systemUptime + 10
        while let event = NSApp.nextEvent(matching:.any,until:.distantPast,inMode:.default,dequeue:true) {
            NSApp.sendEvent(event)
            precondition(ProcessInfo.processInfo.systemUptime < readyBy,"Native event queue did not settle before mouse input")
        }
        window.contentView!.layoutSubtreeIfNeeded()
        let point = view.convert(NSPoint(x:view.bounds.midX,y:view.bounds.midY),to:nil)
        let events: [NSEvent.EventType] = drag ? [.leftMouseDown,.leftMouseDragged,.leftMouseUp] : [.leftMouseDown,.leftMouseUp]
        for (index,type) in events.enumerated() {
            let position = NSPoint(x:point.x + (drag && index > 0 ? 50 : 0),y:point.y)
            let event = NSEvent.mouseEvent(with:type,location:position,modifierFlags:[],timestamp:ProcessInfo.processInfo.systemUptime + Double(index) * 0.02,windowNumber:window.windowNumber,context:nil,eventNumber:index,clickCount:1,pressure:type == .leftMouseUp ? 0 : 1)!
            NSApp.postEvent(event,atStart:false)
        }
        // AppKit's first mouse-down can outlast a short fixed pump interval.
        // Buttons consume mouse-up in their tracking loop, so their action also
        // confirms dispatch. Keep the behavior assertions below independent.
        let before = (opens, completions, toggles)
        let until = ProcessInfo.processInfo.systemUptime + 5
        var dispatched = false
        while !dispatched && ProcessInfo.processInfo.systemUptime < until {
            if let event = NSApp.nextEvent(matching:.any,until:Date().addingTimeInterval(0.005),inMode:.default,dequeue:true) {
                NSApp.sendEvent(event)
                dispatched = event.type == .leftMouseUp && event.windowNumber == window.windowNumber
            }
            RunLoop.main.run(until:Date().addingTimeInterval(0.005))
            dispatched = dispatched || (opens, completions, toggles) != before
        }
        precondition(dispatched, "Native mouse sequence did not finish dispatching")
    }
    snapshot(nil)
    click(card.body)
    precondition(opens == 1 && completions == 1, "Native body mouse click was swallowed")
    click(card.header)
    precondition(opens == 2 && completions == 2, "Title click conflicts with dragging")
    click(card.header,drag:true)
    precondition(opens == 2, "Dragging must not open a notice")
    click(card.link!)
    precondition(opens == 3 && completions == 3, "CTA fired twice through the parent click handler")
    click(card.close)
    precondition(opens == 3 && completions == 4, "Dismiss must not open the notice")
    card.view.removeFromSuperview()
    let group = ProjectGroup("Clicks",toggle:{ toggles += 1 },clear:{})
    group.update([card],expanded:false)
    window.contentView!.addSubview(group.view)
    snapshot(false)
    click(group.summary)
    precondition(toggles == 1, "Group summary must expand on click")
    group.update([card],expanded:true)
    snapshot(true)
    click(card.body)
    precondition(opens == 4 && toggles == 1, "Nested card click must not collapse its group")
    window.close()
    print("Passed: native mouse events open body/title, preserve dragging, avoid duplicate CTA/dismiss actions, expand groups and isolate nested card clicks")
}

// Run every imported document and hand-built renderer node through AppKit.
func auditPoeMarkdownCorpus(_ path: String) {
    let data=try! Data(contentsOf:URL(fileURLWithPath:path))
    let corpus=try! JSONSerialization.jsonObject(with:data) as! [String:Any]
    var cases=corpus["documents"] as! [[String:Any]]
    func adapt(_ original: [String:Any]) -> [String:Any] {
        var node=original
        node["lineStart"]=1; node["lineEnd"]=1
        let type=node["type"] as! String
        var children=(node["children"] as? [[String:Any]] ?? []).map(adapt)
        if type=="alert" {node["value"]=(node["kind"] as? String)?.capitalized}
        if type=="footnoteReference" || type=="footnoteDefinition" {node["value"]=node["label"]}
        if type=="image", let alt=node["alt"] as? String {children=[adapt(["type":"text","value":alt])]}
        if type=="listItem", let checked=node["checked"] as? Bool {children.insert(adapt(["type":"task","checked":checked]),at:0)}
        if type=="list", node["ordered"] as? Bool == false {node.removeValue(forKey:"start")}
        if type=="list", node["ordered"] as? Bool == true && node["start"]==nil {node["start"]=1}
        if type=="table", let align=node["align"] as? [Any] {node["align"]=align.map { $0 as? String ?? "left" }}
        if !children.isEmpty {node["children"]=children}
        return node
    }
    for fixture in corpus["renderNodes"] as! [[String:Any]] {
        cases.append(["name":fixture["test"]!,"ast":adapt(fixture["ast"] as! [String:Any])])
    }
    let pasteboard=NSPasteboard(name:.init("hey-boss-poe-corpus-"+UUID().uuidString))
    defer {pasteboard.releaseGlobally()}
    var count=0
    for fixture in cases {
        autoreleasepool {
            let bytes=try! JSONSerialization.data(withJSONObject:fixture["ast"]!)
            let node=try! JSONDecoder().decode(NativeMarkdownNode.self,from:bytes)
            let rendered=NativeMarkdownRenderer.render(node)
            let text=DocumentText(frame:NSRect(x:0,y:0,width:720,height:640))
            text.isEditable=false; text.isSelectable=true
            text.textStorage!.setAttributedString(rendered)
            text.copyPasteboard=pasteboard
            text.selectAll(nil)
            if rendered.length>0 {
                text.copy(nil)
                precondition(pasteboard.string(forType:.string) != nil,"Copy failed: \(fixture["name"]!)")
                precondition(pasteboard.data(forType:.rtf) != nil,"Rich copy failed: \(fixture["name"]!)")
            }
            func literals(_ node: NativeMarkdownNode, insideTable: Bool = false) -> [String] {
                if ["tableRow","tableCell"].contains(node.type) && !insideTable {return []}
                if ["frontmatter","image"].contains(node.type) {return []}
                if ["text","html","inlineCode","code"].contains(node.type) {return [node.value ?? ""]}
                return (node.children ?? []).flatMap { literals($0,insideTable:insideTable || node.type == "table") }
            }
            let string=rendered.string as NSString
            var cursor=0
            for value in literals(node) where !value.isEmpty {
                let found=string.range(of:value,range:NSRange(location:cursor,length:string.length-cursor))
                precondition(found.location != NSNotFound,"Native rendering lost text \(value.debugDescription) in \(fixture["name"]!)")
                cursor=NSMaxRange(found)
            }
            count += 1
        }
    }
    print("Passed: all \(count) upstream document/renderer inputs retain text order and native plain/rich selection copying")
}

func auditArtifactLibrary() {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("hb-library-tests-" + UUID().uuidString)
    try! FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let cli = root.appendingPathComponent("fixture")
    try! Data("""
    #!/bin/sh
    case "$*" in
      *projects*) echo '{"projects":[{"id":"named:Atlas","name":"Atlas"},{"id":"named:Studio","name":"Studio"}]}' ;;
      *slow*) sleep 0.3; echo '{"artifacts":[{"id":"stale","title":"Stale response"}]}' ;;
      *"--offset 2"*) echo '{"artifacts":[{"id":"page-two","title":"An older document"}],"more":false}' ;;
      *missing*) echo '{"artifacts":[]}' ;;
      *failure*) exit 1 ;;
      *Studio*) echo '{"artifacts":[{"id":"studio","title":"A different project","preview":"Studio document preview","updated_at":1792079305000}]}' ;;
      *) echo '{"artifacts":[{"id":"atlas","title":"A thoughtful release","preview":"Bring the details together. A calm space for plans, decisions, and the work ahead.","updated_at":1792079305000},{"id":"empty","title":"A fresh page","preview":"","updated_at":1792079305000}],"more":true}' ;;
    esac
    """.utf8).write(to: cli)
    try! FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: cli.path)
    let editor = NativeArtifactEditor(launch: ArtifactLaunch(project: "named:Atlas", host: "test-host"), journalRoot: root, cli: cli.path, present: false)
    defer { editor.window.delegate = nil; editor.window.close() }
    func wait(_ condition: () -> Bool) {
        let deadline = Date().addingTimeInterval(5)
        while !condition() && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.01)) }
        precondition(condition(), "Library operation completed")
    }
    wait { editor.documents.count == 2 && editor.projectPicker.numberOfItems == 2 }
    precondition(!editor.gallery.isHidden && editor.page.isHidden, "Home displays documents instead of a blank editor")
    precondition(editor.documents[0].preview?.contains("calm space") == true, "List previews reach the native gallery without per-document reads")
    precondition(Calendar.current.component(.year, from: editor.documents[0].updatedDate!) == 2026, "Artifact timestamps use milliseconds")
    editor.moreDocuments()
    wait { editor.documents.count == 3 }
    precondition(editor.documents.last?.id == "page-two" && !editor.libraryMore, "Pagination keeps older documents reachable")
    var opened: [ArtifactLaunch] = []
    editor.onOpen = { opened.append($0) }
    editor.gallery.cards[0].performClick(nil)
    precondition(opened.count == 1 && opened[0].id == "atlas" && opened[0].host == "test-host")
    editor.projectPicker.selectItem(at: 1); editor.selectProject()
    wait { editor.documents.first?.id == "studio" }
    editor.newDocument()
    precondition(opened.last?.project == "named:Studio" && opened.last?.host == "test-host", "New documents use the selected project and authority")
    precondition(editor.session.launch.project == "named:Atlas", "Browsing another project never retargets an editing session")
    editor.search.stringValue = "slow"; editor.refreshLibrary()
    editor.search.stringValue = ""; editor.refreshLibrary()
    wait { !editor.libraryLoading }
    RunLoop.main.run(until: Date().addingTimeInterval(0.4))
    precondition(editor.documents.first?.id == "studio", "Late search responses cannot replace the current project or query")
    editor.search.stringValue = "missing"; editor.refreshLibrary()
    wait { !editor.libraryLoading }
    precondition(editor.documents.isEmpty && editor.gallery.message.stringValue == "No matching documents")
    editor.search.stringValue = "failure"; editor.refreshLibrary()
    wait { !editor.libraryLoading }
    precondition(editor.gallery.message.stringValue == "Couldn’t load documents" && !editor.gallery.retry.isHidden, "Failures are visible and retryable")
    editor.search.stringValue = ""; editor.refreshLibrary()
    wait { editor.documents.count == 1 }
    if let directory = ProcessInfo.processInfo.environment["HEY_BOSS_LIBRARY_SCREENSHOTS"] {
        try! FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
        editor.projectPicker.selectItem(at: 0); editor.selectProject()
        wait { editor.documents.count == 2 }
        let samples = editor.documents
        for (name, width, appearance) in [("light", 1100.0, NSAppearance.Name.aqua), ("dark", 1100.0, .darkAqua), ("compact", 720.0, .aqua), ("empty", 960.0, .aqua), ("search", 720.0, .aqua), ("error", 720.0, .darkAqua), ("long", 720.0, .darkAqua)] {
            editor.documents = ["empty", "search", "error"].contains(name) ? [] : samples
            editor.libraryMore = ["light", "dark", "compact"].contains(name)
            editor.search.stringValue = name == "search" ? "unmatched" : ""
            editor.libraryFailed = name == "error"
            if name == "long" {
                editor.documents = [ArtifactLibraryDocument(id: "long", title: "A long title about planning a thoughtful release across every connected device 🦀", preview: String(repeating: "Keep every detail clear, even when ideas take a little more room. ", count: 6), updated_at: nil)]
            }
            editor.table.reloadData(); editor.renderLibrary()
            editor.window.appearance = NSAppearance(named: appearance)
            editor.window.setContentSize(NSSize(width: width, height: 740)); editor.window.orderFront(nil)
            editor.window.contentView!.layoutSubtreeIfNeeded()
            RunLoop.main.run(until: Date().addingTimeInterval(0.1))
            let view = editor.window.contentView!, bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
            view.cacheDisplay(in: view.bounds, to: bitmap)
            try! bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: directory).appendingPathComponent(name + ".png"))
            precondition(editor.gallery.cards.allSatisfy { $0.frame.maxX <= editor.gallery.scroll.contentView.bounds.width }, "Cards fit the actual viewport, including legacy scrollbars")
        }
    }
    editor.session.adopt(ArtifactDocument(id: "atlas", title: "A thoughtful release", body: "Original", version: 1))
    editor.loaded()
    editor.session.writer = { _, _ in }
    editor.text.string = "Unsent changes"; editor.session.change(title: "A thoughtful release", body: editor.text.string)
    editor.showLibrary()
    editor.libraryProject = "named:Atlas"; editor.openDocument("atlas")
    precondition(editor.gallery.isHidden && editor.text.string == "Unsent changes" && editor.session.dirty, "Returning to an open document preserves its draft")
    editor.session.timer?.invalidate()
    print("PASS native library previews, project routing, empty/error recovery, and responsive layout")
}

func auditArtifactEditor() {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("hb-editor-tests-" + UUID().uuidString)
    try! FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let launch = ArtifactLaunch(project: "named:Editor tests", id: "test-doc")
    let session = ArtifactEditingSession(launch: launch, journalRoot: root)
    precondition(!session.dirty, "An unopened library or loading document has nothing to save")
    session.adopt(ArtifactDocument(id: "test-doc", title: "Notes", body: "original", version: 4))
    var requests: [ArtifactSave] = []
    var callbacks: [(Result<ArtifactDocument, Error>) -> Void] = []
    session.writer = { request, completion in requests.append(request); callbacks.append(completion) }
    session.change(title: "Notes", body: "first edit")
    session.flush()
    precondition(requests.count == 1 && requests[0].version == 4)
    session.change(title: "Notes", body: "typed during save")
    session.flush()
    precondition(requests.count == 1, "only one write in flight")
    callbacks.removeFirst()(.success(ArtifactDocument(id: "test-doc", title: "Notes", body: "first edit", version: 5)))
    session.flush()
    precondition(requests.count == 2 && requests[1].version == 5 && requests[1].body == "typed during save")
    callbacks.removeFirst()(.failure(StorageError(description: "Disconnected")))
    let retry = requests[1].requestID
    session.flush()
    precondition(requests[2].requestID == retry, "uncertain writes reuse their durable request ID")
    callbacks.removeFirst()(.success(ArtifactDocument(id: "test-doc", title: "Notes", body: "typed during save", version: 6)))
    precondition(!session.dirty && session.version == 6)
    session.change(title: "", body: "validation draft"); session.flush()
    session.change(title: "Corrected title", body: "validation draft")
    callbacks.removeFirst()(.failure(ArtifactWriteFailure(code: "invalid_input", message: "Title cannot be empty")))
    precondition(requests.last?.title == "Corrected title", "A rejected invalid snapshot cannot pin later valid edits")
    callbacks.removeFirst()(.success(ArtifactDocument(id: "test-doc", title: "Corrected title", body: "validation draft", version: 7)))
    precondition(!session.dirty && session.version == 7)
    let cleanReopen = ArtifactEditingSession(launch: launch, journalRoot: root)
    precondition(!cleanReopen.ready && !cleanReopen.dirty, "A clean recovery snapshot must reload the current document")
    session.change(title: "Recovered", body: "unsent 🦀 draft")
    try! session.persist()
    let recovered = ArtifactEditingSession(launch: launch, journalRoot: root)
    precondition(recovered.body == "unsent 🦀 draft" && recovered.version == 7 && recovered.dirty)
    session.timer?.invalidate(); recovered.timer?.invalidate()
    // A newly created document keeps its journal identity when the server assigns an ID.
    let newNote = ArtifactEditingSession(launch: ArtifactLaunch(project: "named:Editor tests", title: "New note"), journalRoot: root)
    newNote.adopt(ArtifactDocument(id: "assigned-id", title: "New note", body: "saved", version: 1))
    newNote.change(title: "New note", body: "pending after creation")
    try! newNote.persist()
    let reopened = ArtifactEditingSession(launch: newNote.launch, journalRoot: root)
    precondition(reopened.journal == newNote.journal && reopened.body == "pending after creation")
    newNote.timer?.invalidate(); reopened.timer?.invalidate()
    print("PASS artifact autosave serializes writes, retries idempotently and recovers Unicode drafts")

    let text = ArtifactMarkdownText(frame: NSRect(x: 0, y: 0, width: 700, height: 400))
    text.isRichText = false; text.isEditable = true; text.allowsUndo = true
    text.string = "hello 🦀 world"; text.setSelectedRange(NSRange(location: 6, length: 2))
    text.wrap("**")
    precondition(text.string == "hello **🦀** world" && text.selectedRange() == NSRange(location: 8, length: 2))
    text.wrap("**")
    precondition(text.string == "hello 🦀 world")
    text.string = "one\ntwo\n"; text.setSelectedRange(NSRange(location: 0, length: 7))
    text.prefixLines("- ")
    precondition(text.string == "- one\n- two\n")
    text.prefixLines("- ")
    precondition(text.string == "one\ntwo\n")
    text.string = "# Heading 🦀"; text.setSelectedRange(NSRange(location: 4, length: 0))
    text.prefixLines("## ")
    precondition(text.string == "## Heading 🦀" && text.selectedRange() == NSRange(location: 5, length: 0), "Heading commands convert levels and preserve the caret")
    text.string = "- item"; text.setSelectedRange(NSRange(location: 6, length: 0))
    text.prefixLines("- [ ] ")
    precondition(text.string == "- [ ] item" && text.selectedRange() == NSRange(location: 10, length: 0))
    text.string = "- one\n- two"; text.selectAll(nil); text.prefixLines("1. ")
    precondition(text.string == "1. one\n2. two", "Numbered lists convert existing markers and advance numbers")
    text.prefixLines("1. "); precondition(text.string == "one\ntwo")
    text.string = "**bold**"; text.setSelectedRange(NSRange(location: 2, length: 4)); text.wrap("*")
    precondition(text.string == "***bold***", "Italic adds emphasis inside bold")
    text.wrap("*"); precondition(text.string == "**bold**")
    text.string = ""; text.setSelectedRange(NSRange(location: 0, length: 0)); text.prefixLines("- ")
    text.insertText("new", replacementRange: text.selectedRange())
    precondition(text.string == "- new", "Typing after a list command keeps its marker")
    text.string = "3. third"; text.setSelectedRange(NSRange(location: 8, length: 0)); text.insertNewline(nil)
    precondition(text.string == "3. third\n4. ")
    text.insertNewline(nil)
    precondition(text.string == "3. third\n", "Return exits an empty list item")
    print("PASS Markdown formatting preserves Unicode selections and toggles selected lines")

    text.string = "# Tasks 🦀\n  - [X] nested\n> - [ ] quoted\n7. [ ] numbered\n```md\n- [ ] example\n```\nordinary [ ] text"
    for word in ["nested", "quoted", "numbered"] {
        let offset = (text.string as NSString).range(of: word).location
        text.setSelectedRange(NSRange(location: offset, length: 0))
        precondition(text.toggleTask(at: offset))
        precondition(text.selectedRange().location == offset)
    }
    precondition(text.string.contains("  - [ ] nested") && text.string.contains("> - [x] quoted") && text.string.contains("7. [x] numbered"))
    precondition(!text.toggleTask(at: (text.string as NSString).range(of: "example").location), "Fenced examples cannot be checked")
    precondition(!text.toggleTask(at: (text.string as NSString).range(of: "ordinary").location), "Ordinary brackets are not tasks")
    text.string = "- [x] done 🦀"; text.setSelectedRange(NSRange(location: (text.string as NSString).length, length: 0)); text.insertNewline(nil)
    precondition(text.string == "- [x] done 🦀\n- [ ] ", "Return continues a checked task as unchecked")
    text.insertNewline(nil)
    precondition(text.string == "- [x] done 🦀\n", "Return exits an empty checklist")

    text.string = "Meet 🦀 today"; text.setSelectedRange(NSRange(location: 5, length: 2))
    let link = text.linkSelection()
    precondition(link.label == "🦀" && link.destination.isEmpty)
    text.replace(link.range, with: ArtifactMarkdownText.markdownLink(link.label, "https://example.com/a b?q=(x)"), selection: NSRange(location: 5, length: 0))
    precondition(text.string == "Meet [🦀](<https://example.com/a%20b?q=(x)>) today")
    text.setSelectedRange(NSRange(location: 7, length: 0))
    let existingLink = text.linkSelection()
    precondition(existingLink.label == "🦀" && existingLink.destination == "https://example.com/a%20b?q=(x)")
    precondition(ArtifactMarkdownText.markdownLink(existingLink.label, existingLink.destination) == "[🦀](<https://example.com/a%20b?q=(x)>)", "Editing a link preserves URL escapes")
    precondition(ArtifactMarkdownText.markdownLink("Section", "https://example.com/#section") == "[Section](<https://example.com/#section>)", "Link fragments retain their delimiter")
    precondition(ArtifactMarkdownText.markdownLink("[label]", "https://example.com/>").contains("\\[label\\]"))
    let clipboard = NSPasteboard.withUniqueName()
    clipboard.setString("plain text", forType: .string)
    precondition(ArtifactImageInput.read(clipboard).isEmpty, "Plain text paste retains native editing behavior")
    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 32, pixelsHigh: 24, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    bitmap.setColor(NSColor(deviceRed: 0.2, green: 0.4, blue: 0.9, alpha: 1), atX: 0, y: 0)
    let png = bitmap.representation(using: .png, properties: [:])!
    clipboard.clearContents(); clipboard.setData(png, forType: .png)
    let images = ArtifactImageInput.read(clipboard)
    precondition(images.count == 1 && (try! images[0].contents()).data == png)
    let imported = try! images[0].copy(beside: root.appendingPathComponent("Picture note.md"))
    precondition(imported.hasPrefix("Picture%20note.assets/") && !imported.contains("file:"))
    let copy = root.appendingPathComponent(imported.removingPercentEncoding!)
    precondition(try! Data(contentsOf: copy) == png, "Local images persist beside the Markdown file")
    clipboard.clearContents(); clipboard.writeObjects([copy as NSURL])
    precondition(ArtifactImageInput.containsImages(clipboard) && ArtifactImageInput.read(clipboard).first?.url == copy, "Finder image drops use file URLs")
    let tiff = ArtifactImageInput(url: nil, bytes: bitmap.tiffRepresentation!, name: "Screenshot.tiff")
    precondition(try! tiff.contents().name == "Screenshot.png", "Clipboard TIFF images become web-compatible PNG attachments")
    print("PASS link selection, destination escaping and persistent image paste import")

    let file = root.appendingPathComponent("Keyboard.md")
    try! Data("A native note 🦀\n".utf8).write(to: file)
    let editor = NativeArtifactEditor(launch: ArtifactLaunch(project: "named:Editor tests", file: file.path), journalRoot: root, cli: "/usr/bin/false", present: false)
    func wait(_ condition: () -> Bool) {
        let deadline = Date().addingTimeInterval(5)
        while !condition() && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.01)) }
        precondition(condition(), "Native operation completed")
    }
    wait { editor.session.ready }
    editor.window.makeFirstResponder(editor.text)
    editor.text.setSelectedRange(NSRange(location: 14, length: 2))
    precondition(editor.command("x", shift: true), "Command-Shift-X strikes the selection")
    RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    precondition(editor.text.string == "A native note ~~🦀~~\n")
    precondition(editor.text.layoutManager?.temporaryAttribute(.strikethroughStyle, atCharacterIndex: 16, effectiveRange: nil) as? Int == NSUnderlineStyle.single.rawValue)
    precondition(!editor.command("x", shift: false), "Command-X remains native Cut")
    precondition(editor.command("x", shift: true))
    RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    precondition(editor.text.string == "A native note 🦀\n")
    precondition(editor.text.layoutManager?.temporaryAttribute(.strikethroughStyle, atCharacterIndex: 14, effectiveRange: nil) == nil, "Removing syntax clears the visual strike")
    editor.text.undoManager?.undo()
    precondition(editor.text.string == "A native note ~~🦀~~\n")
    editor.text.undoManager?.undo()
    precondition(editor.text.string == "A native note 🦀\n")
    precondition(editor.command("l", shift: true))
    RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    precondition(editor.text.string == "- [ ] A native note 🦀\n")
    let taskCaret = editor.text.selectedRange()
    let markerGlyphs = editor.text.layoutManager!.glyphRange(forCharacterRange: NSRange(location: 2, length: 3), actualCharacterRange: nil)
    let markerRect = editor.text.layoutManager!.boundingRect(forGlyphRange: markerGlyphs, in: editor.text.textContainer!)
    let clickPoint = editor.text.convert(NSPoint(x: markerRect.midX + editor.text.textContainerOrigin.x, y: markerRect.midY + editor.text.textContainerOrigin.y), to: nil)
    let checkClick = NSEvent.mouseEvent(with: .leftMouseDown, location: clickPoint, modifierFlags: [], timestamp: 0, windowNumber: editor.window.windowNumber, context: nil, eventNumber: 1, clickCount: 1, pressure: 1)!
    editor.text.mouseDown(with: checkClick)
    RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    precondition(editor.text.string == "- [x] A native note 🦀\n" && editor.text.selectedRange() == taskCaret, "Clicking a Markdown checkbox toggles without moving the caret")
    editor.text.undoManager?.undo()
    precondition(editor.text.string == "- [ ] A native note 🦀\n")
    let keyboardCaret = editor.text.selectedRange()
    let checkKey = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [.command], timestamp: 0, windowNumber: editor.window.windowNumber, context: nil, characters: "\r", charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36)!
    precondition(editor.window.performKeyEquivalent(with: checkKey))
    RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    precondition(editor.text.string == "- [x] A native note 🦀\n" && editor.text.selectedRange() == keyboardCaret, "Checking preserves the caret")
    precondition(editor.window.performKeyEquivalent(with: checkKey))
    RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    precondition(editor.text.string == "- [ ] A native note 🦀\n")
    editor.text.undoManager?.undo()
    precondition(editor.text.string.hasPrefix("- [x]"), "Checklist toggles undo independently")
    editor.text.undoManager?.undo(); editor.text.undoManager?.undo()
    precondition(editor.text.string == "A native note 🦀\n")
    precondition(editor.command("l", shift: true))
    RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    let taskNode = NativeMarkdownNode(type: "list", lineStart: 1, lineEnd: 1, children: [
        NativeMarkdownNode(type: "listItem", lineStart: 1, lineEnd: 1, children: [
            NativeMarkdownNode(type: "task", lineStart: 1, lineEnd: 1, checked: false),
            NativeMarkdownNode(type: "text", lineStart: 1, lineEnd: 1, value: "A native note 🦀")
        ])
    ])
    editor.installPreview(taskNode, source: editor.text.string)
    precondition(editor.reader.string.hasPrefix("☐\t"))
    precondition(editor.reader.textStorage!.attribute(.nativeTaskLine, at: 0, effectiveRange: nil) as? Int == 1)
    let previewLink = editor.reader.textStorage!.attribute(.link, at: 0, effectiveRange: nil)!
    precondition(editor.textView(editor.reader, clickedOnLink: previewLink, at: 0))
    RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    precondition(editor.reader.string.hasPrefix("☑\t") && editor.text.string.hasPrefix("- [x]"), "Read-view checkboxes update both source and preview")
    editor.window.makeFirstResponder(editor.reader); editor.reader.setSelectedRange(NSRange(location: 3, length: 0))
    precondition(editor.window.performKeyEquivalent(with: checkKey))
    RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    precondition(editor.reader.string.hasPrefix("☐\t") && editor.text.string.hasPrefix("- [ ]"), "Command-Return also toggles from preview text")
    wait { !editor.session.dirty && !editor.session.saving }
    precondition(try! String(contentsOf: file, encoding: .utf8) == editor.text.string, "Read-view completion autosaves")
    editor.previewSource = "stale"
    precondition(!editor.togglePreviewTask(at: 0), "An outdated preview cannot change another source line")
    editor.text.undoManager?.undo(); editor.text.undoManager?.undo(); editor.text.undoManager?.undo()
    precondition(editor.text.string == "A native note 🦀\n")
    editor.window.makeFirstResponder(editor.text)
    print("PASS strikethrough, clickable checklists, native shortcuts, preview completion and undo")
    if let cli = ProcessInfo.processInfo.environment["HEY_BOSS_CLI_PATH"] {
        let checklistFile = root.appendingPathComponent("A calmer workday.md")
        let checklistSource = "# A calmer workday\n\nKeep the important things in view.\n\n~~Move the meeting to Friday~~\n\n## Today\n\n- [ ] Review the new layout\n- [x] Share the first draft\n  - [ ] Collect feedback 🦀\n\nUse **⌘Return** to check an item, and **⌘⇧L** to start a checklist.\n"
        try! Data(checklistSource.utf8).write(to: checklistFile)
        let checklist = NativeArtifactEditor(launch: ArtifactLaunch(project: "named:Editor tests", file: checklistFile.path), journalRoot: root, cli: cli, present: false)
        wait { checklist.session.ready }
        func snapshot(_ mode: String) {
            guard let directory = ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_SNAPSHOT_DIR"], let view = checklist.window.contentView else { return }
            try! FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
            for (name, appearance) in [("light", NSAppearance.Name.aqua), ("dark", NSAppearance.Name.darkAqua)] {
                checklist.window.appearance = NSAppearance(named: appearance)
                view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
                let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
                view.cacheDisplay(in: view.bounds, to: bitmap)
                try! bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: directory).appendingPathComponent("checklist-\(mode)-\(name).png"))
            }
        }
        snapshot("edit")
        checklist.togglePreview()
        wait { checklist.previewSource != nil }
        snapshot("read")
        let previewStorage = checklist.reader.textStorage!
        var taskIndices: [Int] = []
        previewStorage.enumerateAttribute(.nativeTaskLine, in: NSRange(location: 0, length: previewStorage.length)) { value, range, _ in
            if value != nil { taskIndices.append(range.location) }
        }
        precondition(taskIndices.count == 3, "The shared parser maps nested checklist markers")
        let nested = taskIndices[2]
        let nestedLink = previewStorage.attribute(.link, at: nested, effectiveRange: nil)!
        precondition(checklist.textView(checklist.reader, clickedOnLink: nestedLink, at: nested))
        precondition(checklist.text.string.contains("  - [x] Collect feedback 🦀"), "A nested preview checkbox changes its exact source line")
        wait { !checklist.session.dirty && !checklist.session.saving }
        precondition(try! String(contentsOf: checklistFile, encoding: .utf8) == checklist.text.string)
        checklist.window.makeFirstResponder(checklist.reader)
        precondition(checklist.command("z", shift: false))
        wait { checklist.previewSource == checklistSource }
        precondition(checklist.text.string == checklistSource && (checklist.reader.string as NSString).substring(with: NSRange(location: nested, length: 1)) == "☐", "Undo from Read restores source and preview")
        precondition(checklist.command("z", shift: true))
        wait { checklist.previewSource == checklist.text.string }
        precondition(checklist.text.string.contains("  - [x] Collect feedback 🦀"))
        checklist.session.flush(); wait { !checklist.session.dirty && !checklist.session.saving }
        checklist.window.close()
        print("PASS real Markdown parser, nested preview checkboxes, read-mode undo/redo and persisted completion")
    }
    editor.text.setSelectedRange(NSRange(location: 2, length: 6))
    precondition(editor.command("k", shift: false) && editor.linkDialog != nil)
    precondition(editor.linkDialog?.buttons[0].isEnabled == false, "A link needs a destination")
    editor.linkURLField!.stringValue = "https://example.com"
    editor.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: editor.linkURLField))
    precondition(editor.linkDialog!.buttons[0].isEnabled)
    editor.window.endSheet(editor.linkDialog!.window, returnCode: .alertFirstButtonReturn)
    wait { editor.linkDialog == nil }
    precondition(editor.text.string == "A [native](<https://example.com>) note 🦀\n")
    editor.text.setSelectedRange(NSRange(location: 5, length: 0)); editor.link()
    precondition(editor.linkURLField?.stringValue == "https://example.com")
    editor.window.endSheet(editor.linkDialog!.window, returnCode: .alertSecondButtonReturn)
    wait { editor.linkDialog == nil }
    precondition(editor.text.string == "A [native](<https://example.com>) note 🦀\n", "Cancelling the popup leaves the document unchanged")
    editor.text.undoManager?.undo()
    precondition(editor.text.string == "A native note 🦀\n", "Inserting a link is one undo operation")

    editor.text.importImages?(images, 2)
    editor.text.setSelectedRange(NSRange(location: 0, length: 0)); editor.text.insertText("Before ", replacementRange: editor.text.selectedRange())
    wait { editor.imageQueue.isEmpty }
    precondition(editor.text.string.hasPrefix("Before A \n![Pasted image.png]"), "Image insertion follows edits made while the file is being copied")
    precondition(editor.text.string.contains("Keyboard.assets/"))
    let imageNode = try! JSONDecoder().decode(NativeMarkdownNode.self, from: JSONSerialization.data(withJSONObject: ["type": "image", "lineStart": 1, "lineEnd": 1, "url": imported, "children": [["type": "text", "value": "Preview", "lineStart": 1, "lineEnd": 1]]]))
    precondition(NativeMarkdownRenderer.render(imageNode).string == "Preview", "Readers without a local image resolver keep alternative text")
    editor.reader.textStorage!.setAttributedString(NativeMarkdownRenderer.render(imageNode, localImages: true))
    editor.loadPreviewImages(generation: editor.renderGeneration)
    wait { !editor.previewImages.isEmpty }
    let attachment = editor.reader.textStorage!.attribute(.attachment, at: 0, effectiveRange: nil) as! NSTextAttachment
    precondition(attachment.image!.size.width == 32, "Reading view loads a real local image")
    editor.text.undoManager?.undo(); editor.text.undoManager?.undo()
    precondition(editor.text.string == "A native note 🦀\n", "Image insertion and intervening typing undo independently")
    editor.text.importImages?([ArtifactImageInput(url: nil, bytes: Data("not an image".utf8), name: "Invalid.png")], 0)
    wait { editor.imageQueue.isEmpty }
    precondition(editor.imageFailure != nil && editor.text.string == "A native note 🦀\n", "Invalid images never insert broken Markdown")
    editor.imageFailure = nil
    wait { !editor.session.dirty && !editor.session.saving }
    precondition(editor.focusMode && editor.sidebar.superview == nil)
    precondition(editor.window.toolbar?.items.count == 6 && editor.titleField.isDescendant(of: editor.window.contentView!) == false, "Document controls belong to the native window toolbar")
    editor.window.makeFirstResponder(editor.text)
    editor.text.setSelectedRange(NSRange(location: 2, length: 6))
    precondition(editor.command("b", shift: false))
    precondition(editor.text.string == "A **native** note 🦀\n")
    editor.text.undoManager?.undo()
    precondition(editor.text.string == "A native note 🦀\n")
    editor.text.undoManager?.redo()
    precondition(editor.text.string == "A **native** note 🦀\n")
    wait { !editor.session.dirty && !editor.session.saving }
    precondition(try! String(contentsOf: file, encoding: .utf8) == editor.text.string, "No manual save is required")
    precondition(editor.command("p", shift: false) && !editor.focusMode && editor.sidebar.superview != nil)
    precondition(editor.command("f", shift: true) && editor.focusMode)
    precondition(editor.command("f", shift: false) && editor.scroll.isFindBarVisible)
    editor.window.makeFirstResponder(editor.text)
    editor.text.setSelectedRange(NSRange(location: 0, length: 0))
    let bulletKey = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [.command, .shift], timestamp: 0, windowNumber: editor.window.windowNumber, context: nil, characters: "*", charactersIgnoringModifiers: "*", isARepeat: false, keyCode: 28)!
    precondition(editor.window.performKeyEquivalent(with: bulletKey) && editor.text.string.hasPrefix("- "), "Shifted number keys reach list shortcuts")
    precondition(editor.window.performKeyEquivalent(with: bulletKey) && !editor.text.string.hasPrefix("- "))
    precondition(editor.command("2", shift: false))
    precondition((editor.text.textStorage?.attribute(.font, at: 3, effectiveRange: nil) as? NSFont)?.pointSize == 22, "Heading typography participates in native text layout")
    editor.text.undoManager?.undo()
    precondition(!editor.text.string.hasPrefix("## "), "Visual styling does not interrupt text undo")
    let large = String(repeating: "A line of Markdown text.\n", count: 42000)
    editor.text.string = large; editor.bodyByteCount = large.utf8.count
    editor.text.setSelectedRange(NSRange(location: (large as NSString).length / 2, length: 0))
    let start = Date()
    for _ in 0..<20 { editor.text.insertText("x", replacementRange: editor.text.selectedRange()) }
    let latency = Date().timeIntervalSince(start) * 1000 / 20
    precondition(latency < 100, "Typing in a 1 MiB note must not stall the main thread")
    print(String(format: "PASS 1 MiB native document: %.2f ms per edit", latency))
    editor.session.flush(); wait { !editor.session.dirty && !editor.session.saving }
    // A concurrent external edit cannot be silently overwritten.
    try! Data("external edit".utf8).write(to: file)
    editor.session.change(title: editor.session.title, body: "my unsent change")
    editor.session.flush()
    wait { editor.session.failure != nil }
    precondition(try! String(contentsOf: file, encoding: .utf8) == "external edit")
    precondition(editor.session.body == "my unsent change")
    editor.session.timer?.invalidate(); editor.window.close()
    let cleanFile = root.appendingPathComponent("Clean reopen.md")
    try! Data("first revision".utf8).write(to: cleanFile)
    let cleanLaunch = ArtifactLaunch(project: "named:Editor tests", file: cleanFile.path)
    let firstOpen = NativeArtifactEditor(launch: cleanLaunch, journalRoot: root, cli: "/usr/bin/false", present: false)
    wait { firstOpen.session.ready }
    firstOpen.window.setFrame(NSRect(x: 80, y: 80, width: 900, height: 680), display: false)
    firstOpen.window.close()
    try! Data("edited elsewhere".utf8).write(to: cleanFile)
    let nextOpen = NativeArtifactEditor(launch: cleanLaunch, journalRoot: root, cli: "/usr/bin/false", present: false)
    wait { nextOpen.session.ready }
    precondition(nextOpen.text.string == "edited elsewhere", "Reopening a saved file reads external changes, not a clean recovery journal")
    precondition(abs(nextOpen.window.frame.width - 900) < 1, "New editor windows restore the last window width")
    nextOpen.window.contentView?.layoutSubtreeIfNeeded()
    precondition(abs(nextOpen.page.frame.width - nextOpen.paper.frame.width) < 1, "Writing uses the available window width")
    let second = NativeArtifactEditor(launch: cleanLaunch, journalRoot: root, cli: "/usr/bin/false", present: false)
    wait { second.session.ready }
    let third = NativeArtifactEditor(launch: cleanLaunch, journalRoot: root, cli: "/usr/bin/false", present: false)
    wait { third.session.ready }
    let manager = ArtifactEditors(); manager.windows = [nextOpen, second, third]
    precondition(manager.cycleWindow(from: nextOpen.window, backwards: false) === second)
    precondition(manager.cycleWindow(from: nextOpen.window, backwards: true) === third)
    precondition(manager.cycleWindow(from: third.window, backwards: false) === nextOpen)
    let beforeCycle = nextOpen.text.string
    precondition(nextOpen.command("`", shift: false) && nextOpen.text.string == beforeCycle, "Command-backtick cycles windows instead of inserting code marks")
    second.window.close()
    precondition(manager.cycleWindow(from: nextOpen.window, backwards: false) === third, "Closed windows are skipped")
    third.window.close()
    nextOpen.window.close()
    print("PASS focus, quick switch, find, undo/redo, automatic disk save and external-edit protection")
}

func auditPaging() {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("hb-page-" + UUID().uuidString)
    try! FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let path = root.appendingPathComponent("history.db").path
    let store = try! Store(path)
    var quiet = true, shown = [String](), removed = [String]()
    store.notificationsMuted = { quiet }
    store.show = { shown.append($0.taskID) }
    store.removeMany = { removed += $0 }
    func call(_ payload: [String: Any]) -> [String: Any] {
        let request = try! JSONDecoder().decode(Request.self, from: JSONSerialization.data(withJSONObject: payload.merging(["sync": false]) { a, _ in a }))
        var fds: [Int32] = [0, 0]; precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
        store.handle(request, Reply(fds[1]))
        return try! JSONSerialization.jsonObject(with: FileHandle(fileDescriptor: fds[0], closeOnDealloc: true).readDataToEndOfFile()) as! [String: Any]
    }
    let origin: [String: Any] = ["cwd":"/tmp", "pid":123, "executable":"/bin/hey-boss", "launchers":[], "agent":["id":"paged-agent", "kind":"codex", "machine":"mac", "host":"mac.local"]]
    func send(_ command: String, page: Bool) -> Record {
        let response = call(["command":command, "project":"Synthetic", "title":"Paging test", "question":"Synthetic", "description":"", "origin":origin, "page":page])
        return try! store.database.get(response["task_id"] as! String)
    }
    let ordinary = send("alert", page: false)
    _ = call(["command":"inbox_mute_agent", "task_id":ordinary.taskID, "muted":true])
    let pages = ["alert", "update", "ask"].map { send($0, page: true) }
    precondition(shown == pages.map(\.taskID), "Pages must bypass quiet hours and muted senders")
    quiet = false; store.mutePendingNotifications(); quiet = true; store.mutePendingNotifications()
    _ = call(["command":"inbox_mute_agent", "task_id":ordinary.taskID, "muted":true])
    precondition(pages.allSatisfy { !removed.contains($0.taskID) }, "Settings changes must not hide pages")
    let restarted = try! Store(path)
    restarted.notificationsMuted = { true }
    var restored = [String](); restarted.show = { restored.append($0.taskID) }; restarted.restore()
    precondition(Set(restored) == Set(pages.map(\.taskID)), "Paging must survive persistence and restart")
    let ui = Interface(present: false); ui.notificationsMuted = { true }
    ui.add(ordinary)
    for page in pages { ui.add(page) }
    ui.layout()
    precondition(ui.cards.count == 2 && ui.current?.taskID == pages[2].taskID, "Native cards and questions must bypass quiet hours")
    _ = call(["command":"hide", "task_id":pages[0].taskID])
    precondition(try! store.database.get(pages[0].taskID).status == "ok", "Pages still support explicit dismissal")
    print("PASS paging: delivery, settings changes, restart, native cards/questions, dismissal")
    auditCompanionNotificationReplay(page: true)
}

func auditQuietHours() {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("hb-quiet-" + UUID().uuidString)
    try! FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    var schedule = NotificationQuietHours(time_zone: "America/Chicago")
    let date = ISO8601DateFormatter()
    for (time, muted) in [("2026-10-02T02:59:59Z", false), ("2026-10-02T03:00:00Z", true), ("2026-10-02T11:59:59Z", true), ("2026-10-02T12:00:00Z", false), ("2026-11-01T07:30:00Z", true)] {
        precondition(schedule.active(at: date.date(from: time)!) == muted)
    }
    schedule.enabled = false
    precondition(!schedule.active(at: date.date(from: "2026-10-02T03:00:00Z")!))
    let store = try! Store(root.appendingPathComponent("history.db").path)
    var muted = true
    store.notificationsMuted = { muted }
    var shown = 0
    store.show = { _ in shown += 1 }
    for kind in ["alert", "update", "prompt", "approval"] {
        var row = Record(taskID: kind, kind: kind, question: "Synthetic", project: "Test", title: "Quiet", description: "", options: [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: 0, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
        row.quietHoursMuted = true
        try! store.database.save(row)
    }
    for command in ["alert", "update", "ask"] {
        let raw: [String: Any] = ["command": command, "project": "Synthetic", "title": "Quiet", "question": "Synthetic", "description": "", "sync": false]
        let request = try! JSONDecoder().decode(Request.self, from: JSONSerialization.data(withJSONObject: raw))
        var fds: [Int32] = [0,0]; precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
        store.handle(request, Reply(fds[1]))
        let input = FileHandle(fileDescriptor: fds[0], closeOnDealloc: true)
        let response = try! JSONSerialization.jsonObject(with: input.readDataToEndOfFile()) as! [String: Any]
        let row = try! store.database.get(response["task_id"] as! String)
        precondition(row.quietHoursMuted == true && row.status == "pending" && row.presentedAt == nil)
    }
    store.restore(); precondition(shown == 0)
    muted = false; store.restore(); precondition(shown == 0)
    precondition(try! store.database.pending().count == 7)
    let ui = Interface(present: false); ui.notificationsMuted = { true }
    ui.add(try! store.database.get("alert")); ui.add(try! store.database.get("prompt"))
    precondition(ui.cards.isEmpty && ui.questions.isEmpty && ui.current == nil)
    print("PASS quiet hours: boundaries, timezone, DST, disabled mode, durable inbox, restart suppression, native surfaces")
}

func auditNotificationAgents() {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("hb-senders-" + UUID().uuidString)
    try! FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let store = try! Store(root.appendingPathComponent("history.db").path)
    func call(_ payload: [String: Any]) -> [String: Any] {
        let request = try! JSONDecoder().decode(Request.self, from: JSONSerialization.data(withJSONObject: payload.merging(["sync": false]) { a, _ in a }))
        var fds: [Int32] = [0, 0]; precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
        store.handle(request, Reply(fds[1]))
        return try! JSONSerialization.jsonObject(with: FileHandle(fileDescriptor: fds[0], closeOnDealloc: true).readDataToEndOfFile()) as! [String: Any]
    }
    func send(_ agent: String, _ machine: String = "mac", _ command: String = "alert") -> Record {
        let origin: [String: Any] = ["cwd":"/tmp/project", "pid":123, "executable":"/bin/hey-boss", "launchers":[], "agent":["id":agent, "kind":"codex", "model":"gpt-6-astra", "machine":machine, "host":"mac.local", "session_id":agent, "source":"CODEX_THREAD_ID", "creation_run":["id":"run-1", "project_id":"named:Trace", "number":1, "started_at":1], "invocation":["offset":42,"call_id":"call-1"]]]
        let reply = call(["command":command,"project":"Trace","title":"Synthetic","question":"Synthetic","description":"Synthetic","origin":origin])
        return try! store.database.get(reply["task_id"] as! String)
    }
    var shown = [String](); store.show = { shown.append($0.taskID) }
    let first = send("agent-1", "mac", "update"), second = send("agent-2")
    let mute = call(["command":"inbox_mute_agent", "task_id":first.taskID, "muted":true])
    precondition(mute["status"] as? String == "ok")
    let suppressed = send("agent-1")
    precondition(shown == [first.taskID,second.taskID], "Muted sender must not show new banners")
    let sameModel = send("agent-2")
    precondition(shown.last == sameModel.taskID && sameModel.agentMuted != true, "Mute must not silence another session of the same model on the same machine")
    precondition(suppressed.status == "pending" && suppressed.agentMuted == true, "Muting keeps history and never answers")
    let question = send("agent-1", "mac", "ask")
    precondition(question.status == "pending" && question.agentMuted == true)
    let otherHost = send("agent-1", "other")
    precondition(shown.last == otherHost.taskID, "Mute identity includes machine")
    let restarted = try! Store(root.appendingPathComponent("history.db").path)
    var restored = [String](); restarted.show = { restored.append($0.taskID) }; restarted.restore()
    precondition(!restored.contains(first.taskID) && !restored.contains(suppressed.taskID))
    _ = call(["command":"inbox_mute_agent", "task_id":first.taskID, "muted":false])
    let unmuted = send("agent-1"); precondition(shown.last == unmuted.taskID)
    precondition(try! store.database.get(suppressed.taskID).agentMuted == true, "Unmute does not replay suppressed notices")
    var links = [URL]()
    let card = Card(first, open: {}, openURL: { links.append($0) }, complete: { _,_ in preconditionFailure("Sender actions must not dismiss") })
    let menu = card.info.menu!
    precondition(menu.items.contains { $0.title.contains("agent-1") })
    precondition(menu.items.contains { $0.title.contains("gpt-6-astra") })
    for title in ["Mute agent", "Open agent", "Steer agent"] {
        let item = menu.items.first { $0.title == title }!
        NSApp.sendAction(item.action!, to: item.target, from: item)
    }
    precondition(links.count == 3 && links.allSatisfy { $0.host == "hey-boss.test" })
    precondition(links[1].fragment!.contains("run=run-1") && links[1].fragment!.contains("at=42"))
    precondition(links[2].fragment!.contains("steer=1"))
    card.configure(grouped: false)
    let senderButtons = card.view.content.subviews.compactMap { $0 as? ActionButton }.filter { $0.title.hasSuffix("agent") }
    precondition(senderButtons.isEmpty, "Agent actions must not consume a footer row")
    precondition(card.view.frame.height == card.body.frame.height + 72 + 34)
    precondition(card.view.bounds.contains(card.info.frame) && !card.info.frame.intersects(card.header.frame))
    precondition(launchDetails(first).string.contains("agent-1") && launchDetails(first).string.contains("gpt-6-astra"))
    let ui = Interface(present: false); var questionLinks = [URL]()
    ui.openURL = { questionLinks.append($0) }; ui.add(question)
    let content = (ui.question.contentView as! Surface).content
    let scroll = content.subviews.compactMap { $0 as? NSScrollView }.first!
    precondition(scroll.frame.minY == 0 && scroll.frame.height == content.bounds.height, "Question has no reserved action footer")
    let info = scroll.documentView!.subviews.compactMap { $0 as? InfoButton }.first!
    let steer = info.menu!.items.first { $0.title == "Steer agent" }!
    NSApp.sendAction(steer.action!, to: steer.target, from: steer)
    precondition(questionLinks.count == 1 && ui.current?.taskID == question.taskID)
    print("PASS sender trace, native web actions, persistent scoped mute, pending questions, unmute without replay")
}

func auditSubscriptionQuota() {
    auditNativeBackdrops()
    let json = #"{"schema_version":1,"account":{"provider":"codex","id":"work"},"state":"ok","updated_at":2000,"data":{"windows":[{"id":"weekly","label":"Weekly · all models","remaining_percent":90,"resets_at":"2099-10-15T16:42:07Z"}]}}"#
    let reading = try! JSONDecoder().decode(SubscriptionQuotaReading.self, from: Data(json.utf8))
    precondition(reading.isFresh(now: 2005))
    precondition(!reading.isFresh(now: 3000), "Old provider readings must not look current")
    precondition(!reading.isFresh(now: 1000), "Future timestamps are not fresh")
    precondition(reading.data!.windows[0].remaining == 90)
    let unknown = try! JSONDecoder().decode(SubscriptionQuotaReading.self, from: Data(json.replacingOccurrences(of: "90", with: "null").utf8))
    precondition(unknown.data!.windows[0].remaining == nil, "Unknown quota is never 100%")
    let invalid = try! JSONDecoder().decode(SubscriptionQuotaReading.self, from: Data(json.replacingOccurrences(of: "90", with: "120").utf8))
    precondition(invalid.data!.windows[0].remaining == nil)
    let stale = try! JSONDecoder().decode(SubscriptionQuotaReading.self, from: Data(json.replacingOccurrences(of: #""state":"ok""#, with: #""state":"stale""#).utf8))
    precondition(!stale.isFresh(now: 2005))
    let expired = try! JSONDecoder().decode(SubscriptionQuotaReading.self, from: Data(json.replacingOccurrences(of: "2099-10-15T16:42:07Z", with: "1970-01-01T00:33:24Z").utf8))
    precondition(!expired.isFresh(now: 2005), "Crossing reset invalidates the previous reading")
    let ui = SubscriptionQuotaController()
    var calls: [[String]] = []
    var replies: [(Result<Data, Error>) -> Void] = []
    ui.runner = { args, reply in calls.append(args); replies.append(reply) }
    ui.open(cli: nil)
    precondition(ui.loading && calls.count == 1 && calls[0].contains("--accounts"))
    ui.refresh(); precondition(calls.count == 1, "Refreshes must coalesce")
    replies.removeFirst()(.success(Data(#"{"schema_version":1,"accounts":[{"provider":"codex","id":"work"},{"provider":"claude","id":"personal"},{"provider":"codex","id":"work"}]}"#.utf8)))
    precondition(calls.count == 3 && ui.rows.count == 2, "Fetch every distinct account, including named subscriptions")
    replies.removeFirst()(.success(Data(json.utf8)))
    replies.removeFirst()(.failure(StorageError(description: "Synthetic failure")))
    precondition(!ui.loading && ui.rows[0].reading != nil && ui.rows[1].reading == nil)
    precondition(ui.rows[1].message == "Quota unavailable. Try refreshing.")
    ui.open(cli: nil); precondition(calls.count == 3, "Reopening uses the short-lived cache")
    ui.refresh()
    replies.removeFirst()(.success(Data(#"{"schema_version":1,"accounts":[{"provider":"codex","id":"work"}]}"#.utf8)))
    precondition(ui.rows[0].reading != nil, "Keep existing cards stable while their quota refreshes")
    precondition(ui.subtitle.stringValue == "Subscription quota", "Refresh cached quotas silently")
    replies.removeFirst()(.failure(StorageError(description: "Synthetic refresh failure")))
    precondition(ui.rows[0].reading != nil && !ui.loading && ui.rows[0].reading?.state == "stale", "A failed refresh preserves last-known quota")
    ui.refresh()
    replies.removeFirst()(.failure(StorageError(description: "Discovery offline")))
    precondition(ui.rows.count == 1 && ui.rows[0].reading != nil, "Discovery failure preserves cached cards")
    ui.refresh()
    replies.removeFirst()(.success(Data(#"{"schema_version":1,"accounts":[]}"#.utf8)))
    precondition(!ui.loading && ui.rows.isEmpty && ui.message == "No subscriptions connected")
    ui.refresh()
    replies.removeFirst()(.success(Data(#"{"schema_version":2,"accounts":[]}"#.utf8)))
    precondition(!ui.loading && ui.message == "Could not load subscriptions")
    let fixture = FileManager.default.temporaryDirectory.appendingPathComponent("quota-cli-" + UUID().uuidString)
    defer { try? FileManager.default.removeItem(at: fixture) }
    try! Data(("#!/bin/sh\nprintf '%s' '" + json + "'\nexit 1\n").utf8).write(to: fixture)
    try! FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: fixture.path)
    let processUI = SubscriptionQuotaController()
    processUI.cli = fixture.path
    var completed = false
    processUI.request(["--provider", "codex", "--account", "work"]) { result in
        let decoded = try! JSONDecoder().decode(SubscriptionQuotaReading.self, from: result.get())
        precondition(decoded.account.id == "work", "Preserve structured readings even when the CLI exits nonzero")
        completed = true
    }
    let deadline = Date().addingTimeInterval(5)
    while !completed && Date() < deadline { _ = RunLoop.main.run(mode: .eventTracking, before: Date().addingTimeInterval(0.01)) }
    precondition(completed, "Background CLI reads must complete while the native menu tracks events")
    let overview = AgentsOverview(present: false)
    precondition(overview.statusMenu.items[1].isSeparatorItem, "Quick add leads directly to navigation without an activity status row")
    overview.updateInboxCount(4)
    precondition(overview.inboxCount == 4 && overview.inboxMenuItem.title == "Inbox · 4 unread…")
    overview.updateInboxCount(0)
    precondition(overview.inboxCount == 0 && overview.inboxMenuItem.title == "Inbox…")
    let item = overview.statusMenu.items.first { $0.view === overview.subscriptionQuota.content }
    precondition(item != nil && item?.submenu == nil, "Quota is visible directly inside the status dropdown")
    var menuCalls = 0
    overview.subscriptionQuota.runner = { _, _ in menuCalls += 1 }
    let opened = ProcessInfo.processInfo.systemUptime
    overview.menuWillOpen(overview.statusMenu)
    precondition(ProcessInfo.processInfo.systemUptime - opened < 0.1 && menuCalls == 1)
    precondition(overview.subscriptionQuota.loading && overview.subscriptionQuota.timer != nil)
    overview.menuDidClose(overview.statusMenu)
    precondition(overview.subscriptionQuota.timer != nil, "Closed menus keep quota warm in the background")
    overview.menuWillOpen(overview.statusMenu)
    precondition(menuCalls == 1, "Reopening during a pending read must coalesce")
    overview.menuDidClose(overview.statusMenu)
    let tracking = AgentsOverview(present: false)
    tracking.subscriptionQuota.runner = { args, reply in
        let payload = args.contains("--accounts") ? #"{"schema_version":1,"accounts":[{"provider":"codex","id":"work"}]}"# : json
        let delivery = Timer(timeInterval: 0.05, repeats: false) { _ in reply(.success(Data(payload.utf8))) }
        RunLoop.main.add(delivery, forMode: .common)
    }
    var tracked = false
    let inspect = Timer(timeInterval: 0.5, repeats: false) { _ in
        let quota = tracking.subscriptionQuota
        precondition(!quota.loading && quota.rows.count == 1, "Quota arrives while the dropdown remains open")
        let menuView = quota.content.window!.contentView!
        let cardRect = quota.content.convert(quota.content.bounds, to: menuView)
        precondition(menuView.bounds.contains(cardRect), "The open menu resizes to fit asynchronous quota content")
        if let directory = ProcessInfo.processInfo.environment["HEY_BOSS_QUOTA_SNAPSHOT_DIR"] {
            let bitmap = menuView.bitmapImageRepForCachingDisplay(in: menuView.bounds)!
            menuView.cacheDisplay(in: menuView.bounds, to: bitmap)
            try! bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: directory).appendingPathComponent("tracking-menu.png"))
        }
        tracked = true; tracking.statusMenu.cancelTracking()
    }
    RunLoop.main.add(inspect, forMode: .common)
    tracking.statusMenu.popUp(positioning: nil, at: NSPoint(x: 100, y: 600), in: nil)
    precondition(tracked && tracking.subscriptionQuota.timer != nil, "Tracking ends cleanly and keeps background refresh")
    let cache = FileManager.default.temporaryDirectory.appendingPathComponent("quota-cache-" + UUID().uuidString)
    defer { try? FileManager.default.removeItem(at: cache) }
    let cached = SubscriptionQuotaController(cacheURL: cache)
    cached.rows = [SubscriptionQuotaRow(account: reading.account, reading: reading)]
    cached.finish()
    let restored = SubscriptionQuotaController(cacheURL: cache)
    precondition(restored.rows.count == 1 && restored.rows[0].reading?.data?.windows[0].remaining == 90, "Cached quota is available before any request after restart")
    var refreshCount = 0
    restored.runner = { _, _ in refreshCount += 1 }
    let reopenStart = ProcessInfo.processInfo.systemUptime
    restored.open(cli: nil)
    precondition(ProcessInfo.processInfo.systemUptime - reopenStart < 0.1 && refreshCount == 1 && restored.rows.count == 1)
    precondition(restored.subtitle.stringValue == "Subscription quota" && restored.loading)
    restored.close(); restored.timer?.fire()
    precondition(refreshCount == 1, "Background refresh coalesces while a request is pending")
    restored.finish(); restored.timer?.fire()
    precondition(refreshCount == 2, "Background refresh runs with the menu closed")
    try! Data("broken cache".utf8).write(to: cache)
    precondition(SubscriptionQuotaController(cacheURL: cache).rows.isEmpty, "Invalid caches recover through normal refresh")
    print("PASS subscription quota: freshness, reset, unknown limits, named accounts, partial failures, cache, empty/error states, async inline menu")
}

func auditNativeBackdrops() {
    for backdrop: NSView & BackdropContent in [VibrantBackdrop(), PlainBackdropContainer()] {
        backdrop.frame = NSRect(x: 0, y: 0, width: 200, height: 120)
        let original = NSView(), replacement = NSView()
        backdrop.contentView = original
        precondition(original.superview === backdrop && original.frame == backdrop.bounds)
        backdrop.setFrameSize(NSSize(width: 320, height: 180))
        precondition(original.frame == backdrop.bounds, "Fallback content must resize with its surface")
        backdrop.contentView = replacement
        precondition(original.superview == nil && replacement.superview === backdrop)
        backdrop.contentView = nil
        precondition(replacement.superview == nil, "Detaching content must not leave an invisible interactive view")
    }
    let surface = Surface(frame: NSRect(x: 0, y: 0, width: 320, height: 180))
    let button = NSButton(title: "Test action", target: nil, action: nil)
    surface.content.addSubview(button)
    surface.drawsSurface = false
    precondition(surface.content.superview === surface)
    surface.drawsSurface = true
    precondition(surface.content.superview != nil && button.superview === surface.content)
    for style in [ButtonStyle.primary, .secondary, .quiet, .link] {
        var invoked = false
        let action = ActionButton("Test action", frame: NSRect(x: 0, y: 0, width: 120, height: 44), style: style) { invoked = true }
        action.performClick(nil)
        precondition(invoked && action.frame.height > 0)
    }
    print("PASS native backdrops: resize, content replacement, detach, surface toggling, button actions")
}

func previewSubscriptionQuota(_ state: String) {
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)
    let overview = AgentsOverview(present: false)
    let ui = overview.subscriptionQuota
    let window = NSPanel(contentRect: ui.content.bounds, styleMask: [.titled, .closable], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    if state.contains("menu") {
        window.title = "Quota menu preview"
        let button = NSPopUpButton(frame: NSRect(x: 24, y: 40, width: 280, height: 32), pullsDown: true)
        overview.statusMenu.insertItem(withTitle: "Open status menu", action: nil, keyEquivalent: "", at: 0)
        button.menu = overview.statusMenu; window.contentView!.addSubview(button)
        app.appearance = NSAppearance(named: state.contains("dark") ? .darkAqua : .aqua)
    } else { window.contentView = ui.content }
    if state.contains("live") {
        overview.cli = ProcessInfo.processInfo.environment["HEY_BOSS_CLI_PATH"]
        if !state.contains("menu") { ui.open(cli: overview.cli) }
        window.center(); window.makeKeyAndOrderFront(nil); app.activate(ignoringOtherApps: true)
    }
    else {
        window.appearance = NSAppearance(named: state.contains("dark") ? .darkAqua : .aqua)
        let now = Date().timeIntervalSince1970
        let reset = ISO8601DateFormatter().string(from: Date().addingTimeInterval(7620))
        func reading(_ provider: String, _ id: String, _ status: String, _ windows: [[String: Any]]) -> SubscriptionQuotaRow {
            let value: [String: Any] = ["schema_version": 1, "account": ["provider": provider, "id": id], "state": status, "updated_at": now - (status == "stale" ? 7200 : 20), "data": ["windows": windows]]
            let reading = try! JSONDecoder().decode(SubscriptionQuotaReading.self, from: JSONSerialization.data(withJSONObject: value))
            return SubscriptionQuotaRow(account: reading.account, reading: reading)
        }
        func limit(_ name: String, _ value: Any) -> [String: Any] { ["label": name, "remaining_percent": value, "resets_at": reset] }
        ui.rows = [reading("codex", "work", "ok", [limit("5-hour · all models", 72), limit("Weekly · all models", 18)]), reading("claude", "personal", "ok", [limit("5-hour · all models", 0), limit("Weekly · Sonnet", 43)])]
        if state.contains("stale") { ui.rows = [reading("codex", "long-account-name-for-the-design-team-with-extra-detail", "stale", [limit("Weekly · all models and a very long provider-reported limit label", 18), limit("Weekly · unknown limit", NSNull())]), reading("claude", "personal", "disabled", [])] }
        if state.contains("many") { ui.rows = (1...12).map { reading("codex", "subscription-\($0)", "ok", [limit("Weekly · all models", 72)]) } }
        if state.contains("empty") { ui.rows = []; ui.message = "No subscriptions connected" }
        if state.contains("error") { ui.rows = []; ui.message = "Could not load subscriptions" }
        if state.contains("loading") { ui.rows = []; ui.loading = true }
        ui.runner = { _, _ in }; ui.lastLoaded = Date()
        ui.render(); window.makeKeyAndOrderFront(nil); app.activate(ignoringOtherApps: true)
    }
    if let directory = ProcessInfo.processInfo.environment["HEY_BOSS_QUOTA_SNAPSHOT_DIR"] {
        if state == "live" {
            let deadline = Date().addingTimeInterval(40)
            while ui.loading && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
            precondition(!ui.loading && !ui.rows.isEmpty && ui.rows.contains { $0.reading?.isFresh() == true }, "Live quota integration requires a fresh provider reading")
            print("PASS live quota: \(ui.rows.count) accounts, background CLI → native panel")
        }
        window.setContentSize(ui.content.frame.size); window.makeKeyAndOrderFront(nil); ui.content.layoutSubtreeIfNeeded()
        let bitmap = ui.content.bitmapImageRepForCachingDisplay(in: ui.content.bounds)!
        ui.content.cacheDisplay(in: ui.content.bounds, to: bitmap)
        let canvas = NSImage(size: ui.content.bounds.size)
        canvas.lockFocus()
        window.effectiveAppearance.performAsCurrentDrawingAppearance { NSColor.windowBackgroundColor.setFill(); ui.content.bounds.fill() }
        NSImage(cgImage: bitmap.cgImage!, size: ui.content.bounds.size).draw(in: ui.content.bounds)
        canvas.unlockFocus()
        let composite = NSBitmapImageRep(data: canvas.tiffRepresentation!)!
        try! composite.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: directory).appendingPathComponent(state + ".png"))
        window.close(); return
    }
    withExtendedLifetime((overview, window)) { app.run() }
}

func auditHTMLReader() {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("hb-html-native-" + UUID().uuidString)
    try! FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let fixture = URL(fileURLWithPath: FileManager.default.currentDirectoryPath).appendingPathComponent("tests/fixtures/html-render.md")
    let source = (try! String(contentsOf: fixture, encoding: .utf8)) + "\n- [ ] Native checklist\n\n![Local image](pixel.png)\n"
    let pixel = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 2, pixelsHigh: 2, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    try! pixel.representation(using: .png, properties: [:])!.write(to: root.appendingPathComponent("pixel.png"))
    let file = root.appendingPathComponent("HTML.md"); try! Data(source.utf8).write(to: file)
    let cli = ProcessInfo.processInfo.environment["HEY_BOSS_CLI_PATH"]!
    let editor = NativeArtifactEditor(launch: ArtifactLaunch(project: "named:HTML audit", file: file.path), journalRoot: root.appendingPathComponent("drafts"), cli: cli, present: false)
    defer { editor.window.close() }
    func wait(_ label: String, _ condition: () -> Bool) {
        let deadline = Date().addingTimeInterval(20)
        while !condition() && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }
        precondition(condition(), label)
    }
    func js(_ expression: String, frame: WKFrameInfo? = nil, reader: ArtifactHTMLReader? = nil) -> Any? {
        var result: Any?, done = false
        (reader ?? editor.htmlReader).view.evaluateJavaScript(expression, in: frame, in: .page) { value in
            switch value { case .success(let value): result = value; case .failure(let error): print("HTML audit JavaScript: \(error)") }
            done = true
        }
        wait("JavaScript finished", { done }); return result
    }
    wait("Document opens", { editor.session.ready })
    editor.window.makeKeyAndOrderFront(nil); editor.togglePreview()
    wait("Web reader loads", { editor.htmlReader.loaded && editor.htmlReader.previewFrame != nil })
    var ready = false
    let deadline = Date().addingTimeInterval(20)
    while !ready && Date() < deadline {
        ready = js("document.querySelector('.html-preview')?.dataset.state") as? String == "ready"
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    }
    if !ready { print("HTML reader state: \(js("document.body.innerText") ?? "missing")") }
    precondition(ready, "Native HTML frame renders")
    wait("Native preview sizes to its content", { (js("document.querySelector('.html-preview iframe').clientHeight") as? Int ?? 0) > 250 })
    let initialHeight = js("document.querySelector('.html-preview iframe').clientHeight") as! Int
    _ = js("document.querySelector('#extra').style.height='2600px'; document.querySelector('#toggle').click()", frame: editor.htmlReader.previewFrame)
    wait("Native preview grows without a height cap", { (js("document.querySelector('.html-preview iframe').clientHeight") as? Int ?? 0) > 2600 })
    _ = js("document.querySelector('#toggle').click()", frame: editor.htmlReader.previewFrame)
    wait("Native preview shrinks", { (js("document.querySelector('.html-preview iframe').clientHeight") as? Int ?? 0) <= initialHeight + 2 })
    precondition(js("document.querySelector('details summary').textContent") as? String == "How this works")
    precondition(js("document.querySelector('#counter').click(); document.querySelector('#counter').textContent", frame: editor.htmlReader.previewFrame) as? String == "1")
    precondition(js("(() => { try { return !!parent.document.body } catch { return false } })()", frame: editor.htmlReader.previewFrame) as? Bool == false, "HTML cannot access the native reader")
    precondition(js("(() => { try { return !!localStorage } catch { return false } })()", frame: editor.htmlReader.previewFrame) as? Bool == false, "HTML has no persistent storage")
    precondition(js("window.exampleMustNotRun === undefined") as? Bool == true)
    _ = editor.command("f", shift: false)
    precondition(!editor.htmlReader.searchBar.isHidden, "Find remains available in the HTML reader")
    editor.htmlReader.search.stringValue = "selectable"; editor.htmlReader.find()
    precondition(editor.text.string == source && (try! String(contentsOf: file, encoding: .utf8)) == source, "Rendering preserves the saved Markdown")
    wait("Local images still load", { js("document.querySelector('article img')?.naturalWidth") as? Int == 2 })
    let taskLine = source.components(separatedBy: "\n").firstIndex(of: "- [ ] Native checklist")! + 1
    _ = js("window.webkit.messageHandlers.artifactTask.postMessage({line:\(taskLine),checked:true}); true", frame: editor.htmlReader.previewFrame)
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    precondition(editor.text.string == source, "Sandbox cannot invoke native task editing")
    _ = js("document.querySelector('input[data-task-line]').click()")
    wait("Native checklists remain editable", { editor.text.string.contains("- [x] Native checklist") })
    _ = js("document.querySelector('input[data-task-line]').click()")
    wait("Native checklist restores source", { editor.text.string == source })
    if let directory = ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_SNAPSHOT_DIR"] {
        try! FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
        editor.htmlReader.closeFind()
        var done = false
        editor.htmlReader.view.takeSnapshot(with: nil) { image, error in
            if let tiff = image?.tiffRepresentation, let bitmap = NSBitmapImageRep(data: tiff) {
                try! bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: directory).appendingPathComponent("native-html.png"))
            }
            done = true
        }
        wait("Snapshot completes", { done })
    }
    editor.togglePreview()
    precondition(editor.htmlReader.container.isHidden && editor.htmlReader.document == nil, "Leaving Read stops the HTML document")
    let wholePage = try! String(contentsOf: fixture.deletingLastPathComponent().appendingPathComponent("html-page.html"), encoding: .utf8)
    editor.text.setSelectedRange(NSRange(location: 0, length: (editor.text.string as NSString).length))
    editor.text.insertText(wholePage, replacementRange: editor.text.selectedRange())
    editor.togglePreview()
    wait("Whole HTML document opens", { editor.htmlReader.loaded && editor.htmlReader.previewFrame != nil })
    wait("Whole HTML document uses its full height", { (js("document.querySelector('iframe')?.clientHeight") as? Int ?? 0) > 2800 })
    precondition(js("getComputedStyle(document.documentElement).overflowY", frame: editor.htmlReader.previewFrame) as? String == "hidden", "Only the surrounding page scrolls")
    _ = js("document.querySelector('#toggle').click()", frame: editor.htmlReader.previewFrame)
    wait("Whole HTML document shrinks", { (js("document.querySelector('iframe')?.clientHeight") as? Int ?? 0) < 500 })
    precondition(editor.text.string == wholePage, "Whole HTML source survives reading")
    for name in ["page.html", "page.HTM"] {
        let path = root.appendingPathComponent(name)
        try! Data(wholePage.utf8).write(to: path)
        let imported = NativeArtifactEditor(launch: ArtifactLaunch(project: "named:HTML audit", file: path.path), journalRoot: root.appendingPathComponent("import-drafts"), cli: cli, present: false)
        wait("HTML file opens", { imported.session.ready })
        precondition(imported.text.string == wholePage, "HTML file import preserves source")
        imported.window.makeKeyAndOrderFront(nil); imported.togglePreview()
        wait("Imported HTML reader loads", { imported.htmlReader.loaded && imported.htmlReader.previewFrame != nil })
        wait("Imported HTML fills its content height", { (js("document.querySelector('iframe')?.clientHeight", reader: imported.htmlReader) as? Int ?? 0) > 2800 })
        precondition((try! Data(contentsOf: path)) == Data(wholePage.utf8), "Preview never changes the imported file")
        imported.window.close()
    }
    print("PASS native HTML reader, whole pages, uncapped height, sandbox, scripts, images, checklists, source fidelity, Find and teardown")
}

func auditSkillEditor(_ path: String) {
    let launch = try! JSONDecoder().decode(ArtifactLaunch.self, from: Data(contentsOf: URL(fileURLWithPath: path)))
    let root = URL(fileURLWithPath: path).deletingLastPathComponent()
    let editor = NativeArtifactEditor(launch: launch, journalRoot: root.appendingPathComponent("native-drafts"))
    editor.show()
    func wait(_ condition: () -> Bool) {
        let until = Date().addingTimeInterval(45)
        while !condition(), Date() < until { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }
        precondition(condition(), "Native editor did not settle: \(editor.status.stringValue)")
    }
    wait { editor.session.ready && !editor.loading }
    precondition(editor.session.launch.skillSession == launch.skillSession)
    let body = editor.text.string + "\nNative / edit 🦀\n"
    editor.text.string = body
    editor.textDidChange(Notification(name: NSText.didChangeNotification, object: editor.text))
    wait { !editor.session.dirty && !editor.session.saving }
    precondition(editor.session.failure == nil, editor.status.stringValue)
    precondition(editor.status.stringValue.contains("Synced everywhere"))
    if let output = ProcessInfo.processInfo.environment["SKILLS_SCREENSHOTS"], !output.isEmpty {
        for (name, width, appearance) in [("native-light", 1000.0, NSAppearance.Name.aqua), ("native-dark", 1000.0, .darkAqua), ("native-compact", 620.0, .aqua)] {
            editor.window.appearance = NSAppearance(named: appearance)
            editor.window.setContentSize(NSSize(width: width, height: 720))
            RunLoop.main.run(until: Date().addingTimeInterval(0.2))
            editor.window.contentView!.layoutSubtreeIfNeeded()
            let view = editor.window.contentView!.superview!
            let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
            view.cacheDisplay(in: view.bounds, to: bitmap)
            try! bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: output).appendingPathComponent(name + ".png"))
        }
    }
    editor.window.close()
    print("PASS native skill editor opens, autosaves through Rust, syncs the fleet and reports success")
}
