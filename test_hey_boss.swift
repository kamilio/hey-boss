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
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_MINDMAP_ONLY"] == "1" { auditNativeMindmap(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_QUICK_ISSUE_ONLY"] == "1" { auditQuickIssue(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_SECRET_ONLY"] == "1" { auditSecretInput(); return }
    auditQuickIssue()
    auditSecretInput()
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_HEALTH_ONLY"] == "1" { auditMachineHealth(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_PERFORMANCE"] == "1" { auditPerformance(); return }
    if ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_AGENTS_ONLY"] == "1" { _ = auditAgentOverview(); return }
    auditMachineHealth()
    auditNativeMindmap()
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
    precondition(preview.linkButtons.count == 2)
    precondition(preview.text.string.contains("Relative Anchor Unsupported"))
    preview.linkButtons[0].performClick(nil)
    preview.linkButtons[1].performClick(nil)
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
    auditMarkdownWebReader(sample: updates[0])
    auditDocumentComments(root: root, sample: updates[0])
    auditReviewImage(sample: updates[0])
    auditCommentComposer(root: root)
    auditMultilineSourceComment(root: root)
    print("Passed: grouping threshold, CTA dismissal, preview with local/web/unsupported links, project isolation, queued questions, Unicode answers, durable history, Markdown links")
}

func auditNativeMindmap() {
    let viewer = NativeMindmapViewer()
    var browserURLs: [URL] = []
    var destinations: [URL] = []
    viewer.openExternal = { browserURLs.append($0); return true }
    viewer.launcher.probe = { $0(true) }
    viewer.launcher.openURL = { url in
        destinations.append(url)
        viewer.show(URL(string: "about:blank")!)
        return true
    }
    viewer.open(cli: nil)
    precondition(viewer.window.isVisible, "Mindmaps opens a native window")
    precondition(destinations.last?.path == "/mm")
    precondition(destinations.last?.query == "focus=1", "Native viewer requests the focused web map")
    precondition(browserURLs.isEmpty, "Opening Mindmaps must not launch the browser")
    precondition(viewer.browser.configuration.websiteDataStore.isPersistent, "Project selection survives daemon restarts")
    let window = viewer.window
    viewer.window.close()
    viewer.open(cli: nil)
    precondition(viewer.window === window && viewer.window.isVisible, "Reopen reuses the native window")
    let map = URL(string: "http://127.0.0.1:4781/mm#project=named%3AAtlas&node=topic")!
    precondition(viewer.policy(for: map) == .allow, "Map relationships remain native")
    let issue = URL(string: "http://127.0.0.1:4781/#project=named%3AAtlas&issue=1")!
    precondition(viewer.policy(for: issue) == .cancel && browserURLs.last == issue, "Issue links open in the browser")
    let pr = URL(string: "https://github.com/example/repo/pull/1")!
    precondition(viewer.policy(for: pr) == .cancel && browserURLs.last == pr, "PR links open in the browser")
    let count = browserURLs.count
    precondition(viewer.policy(for: URL(string: "file:///tmp/private")!) == .cancel && browserURLs.count == count)
    viewer.window.close()
    print("Passed: native mindmap window, focused URL, persistent project storage, reopen and resource links")
}

func auditQuickIssue() {
    let overview = AgentsOverview(present: false, cli: nil)
    var opened: [URL] = []
    overview.issuesLauncher.probe = { $0(true) }
    overview.issuesLauncher.openURL = { opened.append($0); return true }
    overview.showQuickIssue()
    precondition(opened.last?.fragment == "quick-issue=1")
    let item = overview.statusMenu.items.first { $0.action == #selector(AgentsOverview.showQuickIssue) }!
    precondition(item.keyEquivalent == " " && item.keyEquivalentModifierMask == [.control, .option])
    precondition(NSApp.sendAction(item.action!, to: item.target, from: item))
    precondition(opened.count == 2)
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

func auditInbox(root: URL, sample: Record) {
    let overview = AgentsOverview(present: false, cli: nil)
    var openedInbox = 0, openedIssues = 0, openedMindmaps = 0
    overview.openInbox = { openedInbox += 1 }
    overview.openIssues = { openedIssues += 1 }
    overview.openMindmaps = { openedMindmaps += 1 }
    precondition(overview.inboxMenuItem.menu != nil && overview.issuesMenuItem.menu === overview.inboxMenuItem.menu)
    precondition(overview.mindmapsMenuItem.menu === overview.inboxMenuItem.menu)
    for item in [overview.inboxMenuItem, overview.issuesMenuItem, overview.mindmapsMenuItem] {
        precondition(overview.validateMenuItem(item))
        precondition(NSApp.sendAction(item.action!, to: item.target, from: item))
    }
    precondition(openedInbox == 1 && openedIssues == 1 && openedMindmaps == 1)
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
    launcher.open(cli:nil,page:.mindmaps)
    precondition(urls.last?.path == "/mm" && urls.last?.fragment == nil && launches == 0, "Mindmaps reuses the shared web service")
    var probes = 0
    launcher.probe = { probes += 1; $0(probes > 1) }
    launcher.open(cli: "/usr/bin/true")
    precondition(launches == 1 && urls.count == 4 && !launcher.launching)
    precondition(urls.last?.path == "/" && urls.last?.fragment == nil, "Issues resets the destination")
    launcher.probe = { $0(false) }
    launcher.open(cli: nil)
    precondition(failures.count == 1 && !launcher.launching)
    var ready: ((Bool) -> Void)?
    launcher.probe = { ready = $0 }
    launcher.open(cli: nil); launcher.open(cli: nil, page: .mindmaps)
    ready?(true)
    precondition(urls.count == 5 && urls.last?.path == "/mm" && !launcher.launching, "Repeated menu clicks must open the latest destination without starting duplicate servers")
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
    print("Passed: web Inbox/Issues/Mindmaps menu actions, service reuse/start/errors, grouped review dismissal, hidden stack, question draft preservation")
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
    if let directory = ProcessInfo.processInfo.environment["HEY_BOSS_AUDIT_SNAPSHOT_DIR"] {
        try! FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
        for dark in [false, true] { appearanceSnapshot(sample: sample, custom: problem, directory: directory, dark: dark) }
        for dark in [false, true] { densitySnapshot(custom: problem, directory: directory, dark: dark) }
    }
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

func densitySnapshot(custom: Record, directory: String, dark: Bool) {
    let canvas = AuditCanvas(frame: NSRect(x: 0, y: 0, width: 920, height: 720))
    canvas.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
    func caption(_ text: String, _ x: CGFloat, _ y: CGFloat, size: CGFloat = 12) {
        let label = PlainTextField(labelWithString: text)
        label.font = .systemFont(ofSize: size, weight: .medium)
        label.textColor = .secondaryLabelColor
        label.frame = NSRect(x: x, y: y, width: 850, height: 22)
        canvas.addSubview(label)
    }
    func fixture(_ kind: String, _ title: String, _ message: String, severity: String? = nil, options: [String] = []) -> Record {
        var row = Record(taskID: UUID().uuidString, kind: kind, question: kind == "update" ? "# Report" : message, project: "Atlas", title: title, description: kind == "update" ? message : "", options: options, autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: 0, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
        row.severity = severity
        return row
    }
    caption("hey-boss · notifications", 24, 676, size: 18)
    caption("344 px cards · 8 px gaps", 24, 640)
    let rows = [
        fixture("update", "Review ready", "Three changes are ready for review."),
        fixture("update", "Authentication migration review", "The migration preserves current sessions. Two changes need review before rollout.", severity: "info"),
        fixture("alert", "Dependency warning", "A required package is outdated. Review the compatibility notes before upgrading.", severity: "warning"),
        fixture("update", "Build failed", "Two integration tests failed. Failure logs and a proposed fix are ready.", severity: "error"),
        fixture("alert", "Checks passed", "All 13 tests passed.", severity: "success")
    ]
    var keep: [Card] = []
    var top: CGFloat = 592
    for (index, sample) in rows.enumerated() {
        var row = sample
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
        if index == 0 { precondition(card.view.frame.height <= 106) }
        top -= card.view.frame.height
        card.view.frame.origin = NSPoint(x: 24, y: top)
        card.view.drawsSurface = false
        canvas.addSubview(card.view)
        top -= 8
        keep.append(card)
    }
    caption("Question", 400, 640)
    let ui = Interface(present: false)
    ui.onPresented = { _, _ in }
    let question = fixture("approval", "Release", "Which format should the report use?", options: ["Markdown", "PDF"])
    ui.add(question)
    ui.notificationCount.stringValue = "\(rows.count + 1) notifications"
    ui.stackToolbar.frame.origin = NSPoint(x: 24, y: 600)
    ui.stackToolbar.drawsSurface = false
    canvas.addSubview(ui.stackToolbar)
    let questionView = ui.question.contentView as! Surface
    precondition(questionView.frame.height < 180)
    questionView.drawsSurface = false
    questionView.frame.origin = NSPoint(x: 400, y: 634 - questionView.frame.height)
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
    caption("One-line update: \(Int(keep[0].view.frame.height)) px (previously 123 px)", 400, group.view.frame.minY - 44)
    caption("Summary width: 312 px (previously 254 px)", 400, group.view.frame.minY - 68)
    caption("Actual-size layout capture; solid backing replaces compositor glass.", 24, 20, size: 11)
    let window = NSWindow(contentRect: canvas.bounds, styleMask: [.borderless], backing: .buffered, defer: false)
    window.contentView = canvas
    window.appearance = canvas.appearance
    canvas.layoutSubtreeIfNeeded()
    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 920, pixelsHigh: 720, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    bitmap.size = canvas.bounds.size
    canvas.cacheDisplay(in: canvas.bounds, to: bitmap)
    let path = URL(fileURLWithPath: directory).appendingPathComponent("balanced-\(dark ? "dark" : "light").png")
    try! bitmap.representation(using: .png, properties: [:])!.write(to: path)
    print("Compact layout: update=\(Int(keep[0].view.frame.height))px, two-line=\(Int(keep[1].view.frame.height))px, question=\(Int(questionView.frame.height))px, group=\(Int(group.view.frame.height))px")
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
        ["id": "codex-1", "pid": 101, "kind": "Codex", "cwd": "/Users/example/Workspace/hey-boss", "session_id": "session-codex", "task": "Build a native overview of running agents and their current projects", "update": "I found the new Codex event schema. I’m checking the parser against your live session.", "activity": "Reading session events", "activity_at": now - 8, "state": "Working", "updated_at": now, "evidence": "Session file held open by this process", "git": repository],
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

func auditMobileOutboxRevisions() {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("hb-outbox-" + UUID().uuidString)
    try! FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let store = try! Store(root.appendingPathComponent("history.db").path)
    try! store.database.execute("CREATE TABLE mobile_outbox(id TEXT PRIMARY KEY)")
    let hub = try! MobileHub(store: store, configuration: .init(url: "http://127.0.0.1", token: String(repeating: "x", count: 32)))
    let row = Record(taskID: "race-test", kind: "update", question: "Test", project: "Test", title: "Test", description: "Test", options: [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: 1, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
    try! hub.track(row); try! hub.track(row)
    try! store.database.execute("DELETE FROM mobile_outbox WHERE id='race-test' AND revision=1")
    let statement = try! store.database.statement("SELECT revision FROM mobile_outbox WHERE id='race-test'")
    precondition(sqlite3_step(statement) == SQLITE_ROW && sqlite3_column_int64(statement, 0) == 2)
    sqlite3_finalize(statement)
    print("Passed: existing outbox migration and newer enqueue survives stale network completion")
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

func auditAgentOverview() -> AgentsOverview {
    auditArrivalLayoutBatching()
    auditMobileOutboxRevisions()
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
    precondition(NSApplication.shared.activationPolicy() == .accessory)
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

func auditResilience(root: URL) {
    let menuOverview = AgentsOverview(present: false, cli: nil)
    menuOverview.updateInboxCount(3)
    menuOverview.updateActivity(["macState": "active", "macIdleSeconds": 2.0, "notifyPhone": false])
    precondition(menuOverview.activityBadge.state == .active)
    precondition(menuOverview.activityMenuItem.title.contains("phone pushes paused"))
    menuOverview.updateInboxCount(4)
    precondition(menuOverview.activityBadge.state == .active && menuOverview.inboxCount == 4)
    menuOverview.updateActivity(["macState": "active", "macIdleSeconds": 40.0, "notifyPhone": false])
    precondition(menuOverview.activityBadge.state == .idle)
    menuOverview.updateActivity(["macState": "confirming", "notifyPhone": false])
    precondition(menuOverview.activityBadge.state == .confirming)
    menuOverview.updateActivity(["macState": "away", "notifyPhone": true])
    precondition(menuOverview.activityBadge.state == .away && menuOverview.activityMenuItem.title.contains("phone pushes enabled"))
    menuOverview.updateActivity(["macState": "unknown"])
    precondition(menuOverview.activityBadge.state == .unknown && menuOverview.activityMenuItem.title.contains("routing unknown"))
    precondition(menuOverview.activityBadge.hitTest(NSPoint(x: 3, y: 3)) == nil)
    precondition(MenuActivity.state(["macState": "active", "macIdleSeconds": Double.nan]) == .unknown)
    let presence = MacPresence()
    var sampleTimes: [Double] = []
    for _ in 0..<200 {
        let start = ProcessInfo.processInfo.systemUptime
        if let idle = presence.hidIdleSeconds { precondition(idle.isFinite && idle >= 0) }
        sampleTimes.append((ProcessInfo.processInfo.systemUptime - start) * 1000)
    }
    print("HID presence sampler: p95=\(sampleTimes.sorted()[190]) ms; mean=\(sampleTimes.reduce(0,+)/200) ms")

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

func auditMarkdownWebReader(sample: Record) {
    var row = sample
    row = Record(taskID: "markdown-web-audit", kind: "update", question: "# Report 🌍\n\n| A | B |\n|---|---|\n|one|two|\n\n```text\n" + String(repeating: "x", count: 2000) + "\n```\n\n> [!WARNING]\n> Review carefully.\n\n<script>window.bad = true</script>", project: "Synthetic", title: "Markdown reader", description: "Fixture", options: [], autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: 0, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
    let reader = Preview(row, openURL: { _ in })
    let deadline = Date().addingTimeInterval(25)
    while !reader.readerLoaded && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
    precondition(reader.readerLoaded, "Controlled Markdown web reader must load through installed CLI")
    var result: [String: Any]?
    reader.browser!.evaluateJavaScript("({heading:document.querySelector('h1').textContent,cells:document.querySelectorAll('td').length,alert:!!document.querySelector('.markdown-alert-warning'),scripts:document.querySelectorAll('script').length,overflow:document.documentElement.scrollWidth>document.documentElement.clientWidth})") { value, error in
        precondition(error == nil, "Markdown DOM inspection failed")
        result = value as? [String: Any]
    }
    while result == nil && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
    precondition(result?["heading"] as? String == "Report 🌍")
    precondition(result?["cells"] as? Int == 2)
    precondition(result?["alert"] as? Bool == true)
    precondition(result?["scripts"] as? Int == 0)
    precondition(result?["overflow"] as? Bool == false, "Long code should scroll inside its block")
    for (width, height, appearance) in [(CGFloat(420), CGFloat(500), NSAppearance.Name.aqua), (CGFloat(1000), CGFloat(740), NSAppearance.Name.darkAqua)] {
        reader.setContentSize(NSSize(width: width, height: height))
        reader.appearance = NSAppearance(named: appearance)
        RunLoop.main.run(until: Date().addingTimeInterval(0.1))
        var layout: [String: Any]?
        reader.browser!.evaluateJavaScript("({width:window.innerWidth,overflow:document.documentElement.scrollWidth>document.documentElement.clientWidth,codeScroll:document.querySelector('pre').scrollWidth>document.querySelector('pre').clientWidth,tableCells:document.querySelectorAll('td').length})") { value, error in
            precondition(error == nil)
            layout = value as? [String: Any]
        }
        let layoutDeadline = Date().addingTimeInterval(5)
        while layout == nil && Date() < layoutDeadline { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
        precondition(layout?["width"] as? Int == Int(width), "Web reader must resize with the native window")
        precondition(layout?["overflow"] as? Bool == false)
        precondition(layout?["codeScroll"] as? Bool == true)
        precondition(layout?["tableCells"] as? Int == 2)
    }
    reader.close()
    print("Passed: installed Markdown renderer, WebKit DOM, Unicode, tables, callout, literal HTML, bounded horizontal code scrolling")
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
    reader.showRenderedMarkdown("<html><body><p>Review text</p></body></html>")
    reader.contentView!.layoutSubtreeIfNeeded()
    reader.reviewSidebar!.arrange()
    precondition(reader.reviewSidebar!.bounds.width == 280)
    precondition(reader.browser!.frame.maxX <= reader.reviewSidebar!.frame.minX)
    precondition(reader.reviewSidebar!.entries.subviews.count == 1)
    reader.close()
    print("Passed: durable document comments, synchronous status wakes on first comment while reader stays open, async response payload, synchronous review wait, completion payload, disabled comments omitted, native sidebar geometry")
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
    var result: [String: Any]?
    reader.browser!.evaluateJavaScript("({images:document.images.length,width:document.images[0].naturalWidth,height:document.images[0].naturalHeight})") { value, error in precondition(error == nil); result = value as? [String: Any] }
    while result == nil && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
    precondition(result?["images"] as? Int == 1 && result?["width"] as? Int == 8 && result?["height"] as? Int == 8)
    reader.close()
    let invalid = DocumentAttachment(name: "bad.png", mime: "image/png", data: "not-an-image")
    do { _ = try invalid.validatedImage(); preconditionFailure("Invalid image accepted") } catch {}
    print("Passed: snapshotted image review, decoded image dimensions in WebKit, image comments sidebar, invalid image rejection")
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
    var selected = false
    reader.browser!.evaluateJavaScript("(()=>{const range=document.createRange();range.selectNodeContents(document.querySelector('p'));const selection=window.getSelection();selection.removeAllRanges();selection.addRange(range);return document.querySelectorAll('.token-keyword,.token-constant,.token-comment').length;})()") { value, error in
        precondition(error == nil && (value as? Int ?? 0) >= 3, "Actual web reader must display highlighted code")
        selected = true
    }
    while !selected && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
    precondition(selected)
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
    var marks: Int?
    reader.browser!.evaluateJavaScript("document.querySelectorAll('mark[data-review-id]').length") { value, error in precondition(error == nil); marks = value as? Int }
    while marks == nil && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
    precondition((marks ?? 0) > 0, "Saved selection must be visibly anchored")
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
    var selected = false
    reader.browser!.evaluateJavaScript("(()=>{const a=document.querySelector('[data-source-start=\"3\"]'),b=document.querySelector('[data-source-start=\"4\"]');const r=document.createRange();r.setStartBefore(a);r.setEndAfter(b);const s=window.getSelection();s.removeAllRanges();s.addRange(r);})()") { _, error in precondition(error == nil); selected = true }
    while !selected && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
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
    var opened: [URL] = []; var cancelled = 0
    actions.openWebsite = { url, completion in opened.append(url); completion(true) }
    actions.forward = { host, remote in precondition(host == "devbox" && remote == 4123); return port }
    actions.cancel = { host, local, remote in precondition(host == "devbox" && local == port && remote == 4123); cancelled += 1 }
    func invoke(_ id: String, _ method: String, _ params: [String: Any], owner: String = "devbox", version: Int = 1, generation: String = "first") -> [String: Any] {
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
                return json
            }
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
        }
        preconditionFailure("Desktop action response timed out")
    }
    let capabilities = invoke("caps", "capabilities", [:])["result"] as! [String: Any]
    precondition((capabilities["methods"] as! [String]).contains("browser.request"))
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
    var metrics: [String: Any] = ["click_handler_ms": stats(click), "layout_display_ms": stats(draw), "interaction_metrics": overview.performanceMetrics(), "agents": agents.count, "warm_rebuild_ms": stats(warm), "search_ms": stats(search)]
    if let endpoint = ProcessInfo.processInfo.environment["HEY_BOSS_PERF_HUB"] {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("hb-perf-" + UUID().uuidString)
        try! FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let store = try! Store(root.appendingPathComponent("history.db").path)
        let hub = try! MobileHub(store: store, configuration: .init(url: endpoint, token: String(repeating: "x", count: 32)))
        hub.start()
        Thread.sleep(forTimeInterval: 0.15)
        let semaphore = DispatchSemaphore(value: 0), lock = NSLock()
        var elapsed: Double = -1
        let start = ProcessInfo.processInfo.systemUptime
        store.queue.async { lock.lock(); elapsed = (ProcessInfo.processInfo.systemUptime - start) * 1000; lock.unlock(); semaphore.signal() }
        precondition(semaphore.wait(timeout: .now() + 10) == .success)
        lock.lock(); metrics["store_queue_ms_during_slow_sync"] = elapsed; lock.unlock()
        hub.timer?.cancel()
        hub.networkQueue.sync {}
        RunLoop.main.run(until: Date().addingTimeInterval(2.5))
        withExtendedLifetime((store, hub)) {}
        try? FileManager.default.removeItem(at: root)
    }
    print(String(decoding: try! JSONSerialization.data(withJSONObject: metrics, options: [.sortedKeys]), as: UTF8.self))
}

final class HealthAuditTextView: NSTextView {
    var copiedText = ""
    override func copy(_ sender: Any?) { copiedText = (string as NSString).substring(with: selectedRange()) }
    override func paste(_ sender: Any?) { insertText("pasted", replacementRange: selectedRange()) }
}

func auditMachineHealth() {
    let value: [String: Any] = [
        "observed_at": 1789730000, "last_cleanup_at": 1789730000,
        "metrics": ["disk_path": "/Users/example", "disk_total_bytes": 1_000_000_000_000, "disk_available_bytes": 120_000_000_000, "memory_total_bytes": 24_000_000_000, "memory_available_bytes": 4_000_000_000, "memory_pressure": "Warning", "swap_used_bytes": 14_000_000_000],
        "config": ["automatic": true, "harvest_processes": true, "clean_worktrees": true, "interval_seconds": 300, "process_min_age_seconds": 3600, "observation_seconds": 300, "worktree_min_age_days": 14, "workspace_roots": ["/Users/example/Workspace"]],
        "processes": [["name": "Cloudflare test browser · PID 4200", "detail": "Owner exited; observing before cleanup", "eligible": false], ["name": "Cloudflare test worker · PID 4201", "detail": "Owner exited; no clients; quiet across repeated checks", "eligible": true]],
        "worktrees": [["name": "/Users/example/Workspace/atlas-feature", "detail": "Modified, untracked, or ignored files; preserved", "eligible": false, "worktree": ["path": "/Users/example/Workspace/atlas-feature", "age_seconds": 259200, "repository": "example/atlas", "github_url": "https://github.com/example/atlas"]]],
        "harvested_processes": 12, "removed_worktrees": 1, "errors": [],
        "phase": "Waiting for the next check", "running": false,
        "activity": [["at": 1789730000, "category": "scan", "message": "Checking process owners and connections"], ["at": 1789730001, "category": "worktree", "message": "atlas-feature preserved: commits not merged"]]
    ]
    let data = try! JSONSerialization.data(withJSONObject: value)
    let snapshot = try! HealthSnapshot.decode(data)
    let ui = MachineHealth(present: false)
    ui.render(snapshot)
    precondition(ui.disk.stringValue.contains("120.0 GB"))
    precondition(ui.memory.stringValue.contains("Warning"))
    precondition(ui.automatic.state == .on && ui.table.numberOfRows == 2)
    precondition(ui.kind.selectedSegment == 0)
    // A clean scan must not leave the process tab empty. Ordinary apps are display-only.
    var live = value; live["processes"] = []
    live["process_inventory"] = [["pid": 4547, "parent": 850, "age_seconds": 432000, "cpu_percent": 12.5, "resident_bytes": 1_200_000_000, "executable": "/Applications/Codex Helper (Renderer)"]]
    let liveData = try! JSONSerialization.data(withJSONObject: live)
    ui.render(try! HealthSnapshot.decode(liveData))
    precondition(ui.items.count == 1 && ui.items[0].name.contains("4547") && !ui.items[0].eligible)
    precondition(ui.kind.label(forSegment: 0) == "Processes (1)")
    precondition(ui.table.tableColumn(withIdentifier: .init("memory"))?.isHidden == false)
    ui.table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
    ui.render(try! HealthSnapshot.decode(liveData))
    precondition(ui.table.selectedRow == 0)
    ui.logSearch.stringValue = "no-such-process"; ui.rebuildItems()
    precondition(ui.items.isEmpty && ui.selectedEvent.stringValue.contains("No entries match"))
    ui.logSearch.stringValue = ""; ui.render(snapshot)
    ui.kind.selectedSegment = 2; ui.switchKind()
    precondition(ui.items.first!.detail.contains("not merged"))
    ui.logSearch.stringValue = "connections"; ui.rebuildItems(); precondition(ui.table.numberOfRows == 1)
    ui.table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
    ui.tableViewSelectionDidChange(Notification(name: NSTableView.selectionDidChangeNotification))
    precondition(ui.selectedEvent.stringValue.contains("connections"))
    ui.logSearch.stringValue = ""; ui.rebuildItems()
    ui.kind.selectedSegment = 1; ui.switchKind(); precondition(ui.table.numberOfRows == 1)
    precondition(ui.table.tableColumn(withIdentifier: .init("age"))?.isHidden == false)
    precondition(ui.items[0].worktree?.repository == "example/atlas" && MachineHealth.age(ui.items[0].worktree?.ageSeconds) == "3d")
    precondition(!ui.clean.isEnabled)
    ui.table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
    ui.tableViewSelectionDidChange(Notification(name: NSTableView.selectionDidChangeNotification))
    precondition(ui.clean.isEnabled && ui.openRepository.isEnabled)
    var removal: [String] = []
    ui.runner = { args, complete in removal = args; complete(.success(data)) }
    ui.clean.performClick(nil); RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    precondition(removal == ["remove-worktree", "/Users/example/Workspace/atlas-feature", "--json"])
    ui.kind.selectedSegment = 0; ui.switchKind()
    var calls: [[String]] = []
    ui.runner = { args, complete in calls.append(args); complete(.success(data)) }
    ui.scan.performClick(nil)
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    precondition(calls.last == ["scan", "--json"] && !ui.busy)
    ui.clean.performClick(nil)
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    precondition(calls.last == ["clean", "--json"])
    ui.automatic.performClick(nil)
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    precondition(calls.contains(["disable"]) && calls.last == ["status", "--json"])
    var pending: [String: (Result<Data, Error>) -> Void] = [:]
    ui.runner = { args, complete in pending[args[0]] = complete }
    ui.scanNow(); ui.refresh()
    precondition(ui.busy && pending["scan"] != nil && pending["status"] != nil)
    var running = value; running["running"] = true; running["phase"] = "Inspecting worktrees"
    pending["status"]?(.success(try! JSONSerialization.data(withJSONObject: running)))
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    precondition(ui.busy && ui.currentPhase.stringValue == "Inspecting worktrees")
    pending["scan"]?(.success(data))
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    precondition(!ui.busy && ui.scan.isEnabled)
    ui.runner = { _, complete in complete(.failure(StorageError(description: "synthetic failure"))) }
    ui.scanNow(); RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    precondition(ui.footer.stringValue.contains("failed") && ui.scan.isEnabled && ui.automatic.state == .on)
    ui.render(snapshot)
    ui.updateHosts(["devbox", "mac.local"])
    var remoteCalls: [[String]] = []
    var lateLocal: ((Result<Data, Error>) -> Void)?
    ui.runner = { _, complete in lateLocal = complete }
    ui.refresh()
    ui.runner = { args, complete in remoteCalls.append(args); complete(.success(data)) }
    ui.machine.selectItem(withTitle: "devbox"); ui.switchMachine()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    precondition(remoteCalls.last == ["--host", "devbox", "status", "--json"])
    var oldLocal = value; oldLocal["phase"] = "STALE LOCAL RESPONSE"
    lateLocal?(.success(try! JSONSerialization.data(withJSONObject: oldLocal)))
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    precondition(ui.currentPhase.stringValue != "STALE LOCAL RESPONSE")
    ui.kind.selectedSegment = 1; ui.switchKind()
    ui.table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
    ui.tableViewSelectionDidChange(Notification(name: NSTableView.selectionDidChangeNotification))
    ui.clean.performClick(nil); RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    precondition(remoteCalls.last == ["--host", "devbox", "remove-worktree", "/Users/example/Workspace/atlas-feature", "--json"])
    var refusal = value; refusal["errors"] = ["Open in a process or agent; preserved"]
    ui.runner = { _, complete in complete(.success(try! JSONSerialization.data(withJSONObject: refusal))) }
    ui.cleanNow(); RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    precondition(ui.footer.stringValue.contains("preserved"))
    ui.render(snapshot)
    ui.window.contentView?.layoutSubtreeIfNeeded()
    let frame = ui.table.enclosingScrollView!.frame
    precondition(frame.height > 150 && frame.minY > ui.footer.frame.maxY)
    if let path = ProcessInfo.processInfo.environment["HEY_BOSS_HEALTH_SCREENSHOT"] {
        ui.kind.selectedSegment = 1; ui.switchKind()
        let view = ui.window.contentView!
        let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
        view.cacheDisplay(in: view.bounds, to: bitmap)
        try! bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: path))
    }
    let clipboard = NSPasteboard.withUniqueName()
    defer { clipboard.releaseGlobally() }
    ui.kind.selectedSegment = 1; ui.switchKind()
    ui.table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
    precondition(ui.copySelected(to: clipboard))
    precondition(clipboard.string(forType: .string)?.contains("example/atlas") == true)
    precondition(ui.disk.isSelectable && ui.roots.isSelectable && ui.footer.isSelectable)
    func key(_ value: String, _ code: UInt16) -> NSEvent {
        NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 0, windowNumber: ui.window.windowNumber, context: nil, characters: value, charactersIgnoringModifiers: value, isARepeat: false, keyCode: code)!
    }
    let editor = HealthAuditTextView(frame: NSRect(x: 0, y: 0, width: 100, height: 30))
    editor.string = "copy this text"; ui.window.contentView!.addSubview(editor); ui.window.makeFirstResponder(editor)
    editor.setSelectedRange(NSRange(location: 5, length: 4))
    precondition(ui.window.performKeyEquivalent(with: key("c", 8)))
    precondition(editor.copiedText == "this")
    ui.render(snapshot); precondition(editor.selectedRange() == NSRange(location: 5, length: 4))
    precondition(ui.window.performKeyEquivalent(with: key("v", 9)) && editor.string == "copy pasted text")
    editor.removeFromSuperview(); ui.window.makeFirstResponder(ui.table)
    ui.window.copySelection = { ui.copySelected(to: clipboard) }
    precondition(ui.window.performKeyEquivalent(with: key("c", 8)))
    precondition(ui.window.performKeyEquivalent(with: key("w", 13)))
    precondition(!ui.window.isVisible && ui.timer == nil)
    if let livePath = ProcessInfo.processInfo.environment["HEY_BOSS_HEALTH_LIVE_JSON"] {
        let liveSnapshot = try! HealthSnapshot.decode(Data(contentsOf: URL(fileURLWithPath: livePath)))
        precondition(liveSnapshot.processInventory?.isEmpty == false)
        ui.kind.selectedSegment = 0; ui.logSearch.stringValue = ""; ui.render(liveSnapshot)
        precondition(ui.table.numberOfRows == liveSnapshot.processInventory!.count)
        ui.table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        precondition(ui.copySelected(to: clipboard) && clipboard.string(forType: .string)!.contains("CPU"))
        ui.window.contentView?.layoutSubtreeIfNeeded()
        if let path = ProcessInfo.processInfo.environment["HEY_BOSS_HEALTH_PROCESS_SCREENSHOT"] {
            let view = ui.window.contentView!
            let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
            view.cacheDisplay(in: view.bounds, to: bitmap)
            try! bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: path))
        }
        print("Live process rows rendered: \(ui.table.numberOfRows)")
    }
    print("Passed: machine health metrics, cleanup controls, process/worktree sections, errors, and responsive layout")
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
    let form = SecretForm.decode(request)!
    var result: [String]?
    let prompt = SecretPrompt(request: request, form: form) { result = $0 }
    precondition(!prompt.entries[0].maskedMode && prompt.entries[1].maskedMode && prompt.window.sharingType == .none)
    prompt.entries[0].revealed.stringValue = "synthetic-login"
    prompt.entries[1].masked.stringValue = long
    prompt.validate(); precondition(prompt.submit.isEnabled)
    prompt.entries[1].toggle.state = .on; prompt.entries[1].toggleVisibility()
    precondition(prompt.entries[1].revealed.stringValue == long && prompt.entries[1].masked.stringValue.isEmpty)
    prompt.window.contentView?.layoutSubtreeIfNeeded()
    prompt.entries[1].toggle.state = .off; prompt.entries[1].toggleVisibility()
    precondition(prompt.entries[1].masked.stringValue == long && prompt.entries[1].revealed.stringValue.isEmpty)
    if let path = ProcessInfo.processInfo.environment["HEY_BOSS_SECRET_SCREENSHOT"] {
        prompt.window.contentView?.layoutSubtreeIfNeeded()
        let view = prompt.window.contentView!
        let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
        view.cacheDisplay(in: view.bounds, to: bitmap)
        try! bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: path))
    }
    prompt.finish(); precondition(result == ["synthetic-login",long])
    precondition(prompt.entries.allSatisfy { $0.value.isEmpty } && prompt.completion == nil)
    var cancelled = false
    let discard = SecretPrompt(request: request, form: form) { cancelled = $0 == nil }
    discard.entries[1].masked.stringValue = long; discard.cancel()
    precondition(cancelled && discard.entries.allSatisfy { $0.value.isEmpty })
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
    Darwin.close(peers[0]); RunLoop.main.run(until: Date().addingTimeInterval(0.4))
    precondition(prompts.active.isEmpty && pending.entries.allSatisfy { $0.value.isEmpty })
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
    print("Passed: web Inbox summaries, Markdown, creation links, relationship-only link/unlink, archived linking, read receipts, question read safety, invalid answers, winning answer preservation, cancellation, review comments and finish")
}
