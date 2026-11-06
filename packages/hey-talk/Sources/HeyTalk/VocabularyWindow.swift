import AppKit
import SwiftUI

final class VocabularyWindow {
    private let window = NativeWindow(contentRect: NSRect(x: 0, y: 0, width: 440, height: 480),
                                      styleMask: [.titled, .closable, .resizable, .fullSizeContentView],
                                      backing: .buffered, defer: false)

    init() {
        window.title = "Hey Talk — Vocabulary"
        window.minSize = NSSize(width: 340, height: 320)
        window.center()
    }

    func show(term: String?, command: @escaping ([String], @escaping ([String: Any]) -> Void) -> Void) {
        window.host(VocabularyView(initial: term, command: command))
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }
}

private struct VocabularyView: View {
    let initial: String?
    let command: ([String], @escaping ([String: Any]) -> Void) -> Void
    @State private var words: [String] = []
    @State private var selected: String?
    @State private var spelling = ""
    @State private var busy = true
    @State private var message: String?
    @FocusState private var editing: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("Dictation vocabulary", systemImage: "character.book.closed")
                .font(.title2.weight(.semibold))
            Text("Choose a word or phrase to correct its spelling.")
                .foregroundStyle(.secondary)
            List(words, id: \.self, selection: $selected) { Text($0) }
                .overlay {
                    if words.isEmpty { Text(busy ? "Loading…" : "No saved words").foregroundStyle(.secondary) }
                }
                .disabled(busy)
                .onChange(of: selected) { _, word in spelling = word ?? "" }
            Text("Preferred spelling").font(.headline)
            HStack {
                TextField("Word or phrase", text: $spelling)
                    .focused($editing).disabled(selected == nil || busy).onSubmit(save)
                Button("Save", action: save).disabled(!canSave)
            }
            if let message { Text(message).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true) }
        }
        .padding(22).padding(.top, 18)
        .onAppear {
            command([]) { response in
                words = response["vocabulary"] as? [String] ?? []
                selected = initial.flatMap { words.contains($0) ? $0 : nil } ?? words.first
                spelling = selected ?? ""
                busy = false
                message = response["error"] as? String ?? response["sync_error"] as? String
                editing = initial != nil && selected == initial
            }
        }
    }

    private var canSave: Bool {
        !busy && selected != nil && !spelling.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && spelling != selected
    }

    private func save() {
        guard canSave, let selected else { return }
        do {
            let term = try Vocabulary.term(spelling)
            busy = true; message = nil
            command(["rename", selected, term]) { response in
                busy = false
                guard let updated = response["vocabulary"] as? [String], response["ok"] as? Bool == true else {
                    message = response["error"] as? String ?? "Couldn’t save this spelling."
                    return
                }
                words = updated; self.selected = term; spelling = term
                message = "Saved “\(term)”."
            }
        } catch { message = error.localizedDescription }
    }
}
