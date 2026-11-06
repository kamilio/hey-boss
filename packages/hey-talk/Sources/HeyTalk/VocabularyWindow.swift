import AppKit
import SwiftUI

final class VocabularyWindow {
    private let window = NativeWindow(contentRect: NSRect(x: 0, y: 0, width: 640, height: 560),
                                      styleMask: [.titled, .closable, .resizable, .fullSizeContentView],
                                      backing: .buffered, defer: false)

    init() {
        window.title = "Hey Talk — Vocabulary"
        window.minSize = NSSize(width: 480, height: 380)
        window.setFrameAutosaveName("DictationVocabulary")
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

    @State private var search = ""
    @State private var deleting: String?
    private var matching: [String] { search.isEmpty ? words : words.filter { $0.localizedStandardContains(search) } }

    var body: some View {
        let visible = matching
        VStack(alignment: .leading, spacing: 0) {
            LibraryHeader(subtitle: "The spellings you want Hey Talk to remember.", badge: "Shared vocabulary", symbol: "arrow.triangle.2.circlepath")
            LibraryToolbar(title: "Dictation vocabulary", placeholder: "Search words and phrases", search: $search)
            if visible.isEmpty {
                LibraryEmptyState(symbol: search.isEmpty ? "character.book.closed" : "magnifyingglass",
                                  title: busy ? "Loading vocabulary…" : search.isEmpty ? "No saved words" : "No matching words",
                                  detail: search.isEmpty ? "Say “Add word” followed by a name or phrase." : "Try another spelling.")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollViewReader { reader in
                    LibraryRows {
                        ForEach(visible, id: \.self) { word in
                            LibraryRow(selected: selected == word) {
                                HStack(spacing: 10) {
                                    Image(systemName: "textformat").foregroundStyle(.teal)
                                    if selected == word {
                                        TextField("Preferred spelling", text: $spelling)
                                            .textFieldStyle(.roundedBorder).focused($editing).onSubmit(save)
                                            .accessibilityLabel("Preferred spelling")
                                        Button("Save", action: save).disabled(!canSave)
                                    } else {
                                        Button { select(word) } label: {
                                            Text(word).font(.system(size: 14)).frame(maxWidth: .infinity, alignment: .leading)
                                                .contentShape(Rectangle())
                                        }.buttonStyle(.plain)
                                        Button { select(word) } label: { Image(systemName: "pencil") }
                                            .help("Edit spelling").accessibilityLabel("Edit \(word)")
                                    }
                                    Button { deleting = word } label: { Image(systemName: "trash") }
                                        .help("Delete this entry").accessibilityLabel("Delete \(word)")
                                }.buttonStyle(.bordered).controlSize(.small)
                            }
                            .overlay(alignment: .bottom) {
                                if word != visible.last { Divider().padding(.horizontal, 18) }
                            }
                        }
                    }
                    .onAppear { if let selected { reader.scrollTo(selected) } }
                }
                .disabled(busy)
            }
            Divider().padding(.horizontal, 26)
            HStack(spacing: 8) {
                Image(systemName: "character.book.closed")
                Text(message ?? "Used by speech recognition and AI correction.")
                Spacer()
                Text("\(words.count) \(words.count == 1 ? "entry" : "entries")")
            }.font(.system(size: 11)).foregroundStyle(.secondary)
                .padding(.horizontal, 26).padding(.vertical, 17)
        }
        .tint(.teal)
        .alert("Delete this vocabulary entry?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }), presenting: deleting) { word in
            Button("Delete", role: .destructive) { change(["remove", word], selected: selected == word ? nil : selected, success: "Deleted “\(word)”.") }
            Button("Cancel", role: .cancel) {}
        } message: { word in Text("“\(word)” will be removed from your shared vocabulary.") }
        .onAppear {
            command([]) { response in
                words = response["vocabulary"] as? [String] ?? []
                busy = false
                message = response["error"] as? String ?? response["sync_error"] as? String
                if let initial, words.contains(initial) { select(initial) }
            }
        }
    }

    private func select(_ word: String) {
        selected = word; spelling = word; editing = true; message = nil
    }

    private var canSave: Bool {
        !busy && selected != nil && !spelling.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && spelling != selected
    }

    private func save() {
        guard canSave, let selected else { return }
        do {
            let term = try Vocabulary.term(spelling)
            change(["rename", selected, term], selected: term, success: "Saved “\(term)”.")
        } catch { message = error.localizedDescription }
    }

    private func change(_ args: [String], selected next: String?, success: String) {
        busy = true; message = nil
        command(args) { response in
            busy = false
            guard response["ok"] as? Bool == true, let updated = response["vocabulary"] as? [String] else {
                message = response["error"] as? String ?? "Couldn’t update the vocabulary."
                return
            }
            words = updated
            if args.first == "rename" || selected != next {
                selected = next; spelling = next ?? ""
            }
            message = success
        }
    }
}
