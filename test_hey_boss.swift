import AppKit
import Foundation

func audit() {
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("out/test-\(UUID().uuidString)")
    try! FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try! FileManager.default.removeItem(at: root) }
    let store = Store(root.appendingPathComponent("history.db").path)
    let ui = Interface(present: false)
    store.show = { ui.add($0) }
    store.remove = { ui.remove($0) }
    store.removeMany = { ui.remove($0) }
    ui.onComplete = { store.complete($0, $1) }
    ui.onCompleteMany = { store.complete($0) }
    ui.onPresented = { store.presented($0, $1) }
    func row(_ kind: String, _ project: String, _ options: [String]) -> Record {
        Record(taskID: UUID().uuidString, kind: kind, question: "# Report\n\n[Read more](https://example.com)", project: project, title: "Build complete", description: "All checks passed", options: options, autoclose: nil, linkURL: nil, linkLabel: nil, createdAt: Date().timeIntervalSince1970, presentedAt: nil, expiresAt: nil, status: "pending", result: nil, origin: nil)
    }
    let updates = (0..<3).map { _ in row("update", "Atlas", []) }
    for item in updates.prefix(2) { store.database.save(item); ui.add(item) }
    precondition(ui.projectGroups.isEmpty)
    store.database.save(updates[2])
    ui.add(updates[2])
    precondition(ui.projectGroups.count == 1)
    ui.expandedProjects.insert("Atlas")
    ui.layout()
    precondition(ui.cards.allSatisfy { !$0.view.isHidden })
    ui.cards[0].link!.performClick(nil)
    precondition(store.database.get(updates[0].taskID).status == "ok")
    precondition(ui.cards.count == 2 && ui.projectGroups.isEmpty)
    precondition(ui.previews.count == 1)
    for preview in ui.previews.values { preview.close() }
    let other = row("alert", "Orion", [])
    store.database.save(other)
    ui.add(other)
    store.complete(Array(updates.dropFirst()).map(\.taskID))
    precondition(ui.cards.count == 1 && ui.cards[0].row.project == "Orion")
    precondition(store.database.get(other.taskID).status == "pending")
    let prompt = row("prompt", "Atlas", [])
    let choice = row("approval", "Atlas", ["PDF", "Markdown"])
    for item in [prompt, choice] { store.database.save(item); ui.add(item) }
    precondition(ui.field != nil && ui.current!.taskID == prompt.taskID)
    ui.answer("Résumé α")
    precondition(ui.current!.taskID == choice.taskID)
    ui.answer("Markdown")
    precondition(ui.current == nil && ui.question.contentView == nil)
    let reopened = Database(root.appendingPathComponent("history.db").path)
    precondition(reopened.get(prompt.taskID).result == "Résumé α")
    precondition(reopened.get(choice.taskID).result == "Markdown")
    precondition(reopened.pending().map(\.taskID) == [other.taskID])
    let attributed = markdown("[Read more](https://example.com)", size: 14, color: .labelColor)
    var links = 0
    attributed.enumerateAttribute(.link, in: NSRange(location: 0, length: attributed.length)) { value, _, _ in if value != nil { links += 1 } }
    precondition(links == 1)
    print("Passed: grouping threshold, CTA dismissal, preview, project isolation, queued questions, Unicode answers, durable history, Markdown links")
}
