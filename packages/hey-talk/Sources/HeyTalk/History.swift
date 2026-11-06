import AppKit
import SwiftUI
import TalkCore

final class DictationHistory: ObservableObject {
    @Published private(set) var entries: [Dictation] = []
    @Published private(set) var unsaved: Set<UUID> = []
    @Published private(set) var warning: String?
    @Published private(set) var hasMore = false
    @Published private(set) var loading = false
    private let queue = DispatchQueue(label: "hey-talk.history", qos: .utility)
    private let directory: URL
    private var loadedFirstPage = false
    private var store: HistoryStore? // Access only on queue.

    init(directory: URL = ControlSocket.directory) { self.directory = directory }

    private func database() throws -> HistoryStore {
        if let store { return store }
        let opened = try HistoryStore(directory: directory)
        store = opened
        return opened
    }

    func saveRecording(_ samples: [Float]) {
        queue.async {
            do { try RecordingArchive.save(samples, in: self.directory.appendingPathComponent("recordings")) }
            catch { NSLog("Hey Talk could not save the recording: %@", error.localizedDescription) }
        }
    }
    func save(_ entry: Dictation, completion: @escaping (Bool) -> Void) {
        queue.async {
            let result = Result { try self.database().append(entry) }
            DispatchQueue.main.async {
                let cutoff = HistoryStore.cutoff
                self.entries.removeAll { $0.createdAt < cutoff && !self.unsaved.contains($0.id) }
                self.entries.insert(entry, at: 0)
                switch result {
                case .success: completion(true)
                case .failure:
                    self.unsaved.insert(entry.id)
                    self.warning = "History could not be saved to disk. Copy unsaved text before quitting Hey Talk."
                    completion(false)
                }
            }
        }
    }

    func read(before: UUID? = nil, completion: @escaping (Result<[Dictation], Error>) -> Void) {
        queue.async {
            let result = Result { try self.database().recent(before: before) }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func refresh() {
        // Saves already update this list. Reuse it unless the first page needs rebuilding.
        let cutoff = HistoryStore.cutoff
        if loadedFirstPage, warning == nil, entries.count <= 50,
           entries.last.map({ $0.createdAt >= cutoff }) ?? true { return }
        load(older: false)
    }
    func loadMore() { load(older: true) }
    func finishPendingWrites() { queue.sync {} }
    private func load(older: Bool) {
        guard !loading else { return }
        loading = true
        let before = older ? entries.last(where: { !unsaved.contains($0.id) })?.id : nil
        read(before: before) { result in
            self.loading = false
            switch result {
            case .success(let page):
                let cutoff = HistoryStore.cutoff
                if older {
                    self.entries.removeAll { $0.createdAt < cutoff && !self.unsaved.contains($0.id) }
                    let known = Set(self.entries.map(\.id))
                    self.entries += page.filter { !known.contains($0.id) }
                } else {
                    self.loadedFirstPage = true
                    let newest = page.first?.createdAt ?? cutoff
                    let retained = self.entries.filter { self.unsaved.contains($0.id) || ($0.createdAt >= cutoff && $0.createdAt > newest) }
                    let known = Set(retained.map(\.id))
                    self.entries = retained + page.filter { !known.contains($0.id) }
                }
                self.hasMore = page.count == 50
                if self.unsaved.isEmpty { self.warning = nil }
            case .failure(let error): self.warning = error.localizedDescription
            }
        }
    }
}

final class HistoryWindow {
    private let window: NativeWindow
    init(history: DictationHistory) {
        window = NativeWindow(contentRect: NSRect(x: 0, y: 0, width: 720, height: 640),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                              backing: .buffered, defer: false)
        window.title = "Hey Talk — Dictation History"
        window.minSize = NSSize(width: 480, height: 400)
        window.host(HistoryView(history: history))
        window.setFrameAutosaveName("DictationHistory")
        window.center()
    }
    func show(keepingFocus: Bool) {
        if keepingFocus { window.orderFrontRegardless(); return }
        window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
    }
}

private struct HistoryView: View {
    @ObservedObject var history: DictationHistory
    @State private var search = ""
    private var matching: [Dictation] {
        search.isEmpty ? history.entries : history.entries.filter {
            $0.text.localizedStandardContains(search) || $0.originalText?.localizedStandardContains(search) == true
        }
    }
    var body: some View {
        let visible = matching
        VStack(alignment: .leading, spacing: 0) {
            LibraryHeader(subtitle: "Your words, always within reach.", badge: "On this Mac", symbol: "lock.shield")
            LibraryToolbar(title: "Dictation history", placeholder: "Search loaded dictations", search: $search)
            if let warning = history.warning {
                Label(warning, systemImage: "exclamationmark.triangle")
                    .font(.callout).foregroundStyle(.orange).padding(16)
            }
            if visible.isEmpty {
                VStack(spacing: 12) {
                    LibraryEmptyState(symbol: search.isEmpty ? "text.bubble" : "magnifyingglass",
                                      title: history.loading ? "Loading history…" : search.isEmpty ? "Your dictations will appear here" : "No matching dictations",
                                      detail: search.isEmpty ? "Press \(DictationShortcut.hint) to start. Press again to finish." : "Try another phrase or load older dictations below.")
                    if history.hasMore { loadMore }
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                LibraryRows {
                    ForEach(visible) { entry in
                        DictationRow(entry: entry, saved: !history.unsaved.contains(entry.id))
                            .overlay(alignment: .bottom) {
                                if entry.id != visible.last?.id { Divider().padding(.horizontal, 18) }
                            }
                    }
                    if history.hasMore { loadMore.padding(16) }
                }
            }
            Divider().padding(.horizontal, 26)
            HStack(spacing: 8) {
                Image(systemName: "keyboard")
                Text(DictationShortcut.hint).fontWeight(.medium)
                Text("to start / stop")
                Spacer()
                Text("Last 5 microphone recordings kept locally")
            }.font(.system(size: 11)).foregroundStyle(.secondary)
                .padding(.horizontal, 26).padding(.vertical, 17)
        }
        .tint(.teal)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
    private var loadMore: some View {
        Button(history.loading ? "Loading…" : "Load older dictations") { history.loadMore() }
            .disabled(history.loading)
    }
}

private struct DictationRow: View {
    let entry: Dictation
    let saved: Bool
    @State private var copied = false
    @State private var copiedOriginal = false
    @State private var showFullText = false
    var body: some View {
        LibraryRow { rowContent }
    }
    private var rowContent: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 7) {
                Image(systemName: entry.source == "file" ? "doc.waveform" : "mic")
                    .foregroundStyle(.teal)
                Text(entry.createdAt.formatted(date: .abbreviated, time: .shortened))
                Text("· \(Int(entry.audioSeconds.rounded()))s")
                if !saved { Text("· Unsaved").foregroundStyle(.orange) }
                Spacer()
                Button {
                    NSPasteboard.general.clearContents()
                    copied = NSPasteboard.general.setString(entry.text, forType: .string)
                } label: {
                    Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc")
                        .frame(minWidth: 60)
                }
                .buttonStyle(.bordered).controlSize(.small)
                .accessibilityLabel(copied ? "Transcript copied" : "Copy transcript")
                .task(id: copied) {
                    if copied {
                        try? await Task.sleep(for: .seconds(2))
                        copied = false
                    }
                }
            }.font(.system(size: 11)).foregroundStyle(.secondary)
            let preview = entry.text.prefix(600)
            let truncated = preview.endIndex != entry.text.endIndex
            TranscriptText(text: truncated && !showFullText ? String(preview) + "…" : entry.text)
                .frame(maxWidth: .infinity, alignment: .leading)
            if truncated {
                Button(showFullText ? "Show less" : "Show full transcript") { showFullText.toggle() }
                    .buttonStyle(.link).font(.callout)
            }
            if let original = entry.originalText {
                if original != entry.text {
                    DisclosureGroup {
                        VStack(alignment: .leading, spacing: 10) {
                            HStack {
                                Text("Before AI correction").foregroundStyle(.secondary)
                                Spacer()
                                Button {
                                    NSPasteboard.general.clearContents()
                                    copiedOriginal = NSPasteboard.general.setString(original, forType: .string)
                                } label: {
                                    Label(copiedOriginal ? "Copied" : "Copy original", systemImage: copiedOriginal ? "checkmark" : "doc.on.doc")
                                }
                                .buttonStyle(.bordered).controlSize(.small)
                                .task(id: copiedOriginal) {
                                    if copiedOriginal {
                                        try? await Task.sleep(for: .seconds(2))
                                        copiedOriginal = false
                                    }
                                }
                            }
                            TranscriptText(text: original).frame(maxWidth: .infinity, alignment: .leading)
                        }.padding(.top, 8)
                    } label: { Text("Original transcript") }
                    .font(.system(size: 11))
                } else {
                    Text("Original unchanged").font(.system(size: 11)).foregroundStyle(.secondary)
                }
            } else {
                Text("Original not saved for this older entry").font(.system(size: 11)).foregroundStyle(.secondary)
            }
        }
    }
}

/// AppKit owns selection and the responder chain. SwiftUI's selectable Text can
/// loop in SelectionOverlay layout on macOS 26 with long, wrapping transcripts.
private struct TranscriptText: NSViewRepresentable {
    let text: String
    func makeNSView(context: Context) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: "")
        label.isSelectable = true
        label.font = .systemFont(ofSize: 14)
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return label
    }
    func updateNSView(_ label: NSTextField, context: Context) {
        if label.stringValue != text { label.stringValue = text }
    }
    func sizeThatFits(_ proposal: ProposedViewSize, nsView label: NSTextField, context: Context) -> CGSize? {
        guard let width = proposal.width, width > 0, width.isFinite, let cell = label.cell else { return nil }
        let size = cell.cellSize(forBounds: NSRect(x: 0, y: 0, width: width, height: .greatestFiniteMagnitude))
        return CGSize(width: width, height: ceil(size.height))
    }
}
