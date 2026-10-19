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
    private var store: HistoryStore? // Access only on queue.

    init(directory: URL = ControlSocket.directory) { self.directory = directory }

    private func database() throws -> HistoryStore {
        if let store { return store }
        let opened = try HistoryStore(directory: directory)
        store = opened
        return opened
    }

    func save(_ entry: Dictation, completion: @escaping (Bool) -> Void) {
        queue.async {
            let result = Result { try self.database().append(entry) }
            DispatchQueue.main.async {
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

    func refresh() { load(older: false) }
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
                // Saves can finish while a read is in flight; retain newer entries.
                if older {
                    let known = Set(self.entries.map(\.id))
                    self.entries += page.filter { !known.contains($0.id) }
                } else {
                    let newest = page.first?.createdAt ?? .distantPast
                    let retained = self.entries.filter { self.unsaved.contains($0.id) || $0.createdAt > newest }
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
    private let window: NSWindow
    init(history: DictationHistory) {
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 640, height: 580),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                          backing: .buffered, defer: false)
        window.title = "Hey Talk — Dictation History"
        window.titlebarAppearsTransparent = true
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 440, height: 360)
        window.contentView = NSHostingView(rootView: HistoryView(history: history))
        window.setFrameAutosaveName("DictationHistory")
        window.center()
    }
    func show() { window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true) }
}

private struct HistoryView: View {
    @ObservedObject var history: DictationHistory
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 7) {
                HStack(spacing: 10) {
                    Image(systemName: "waveform").foregroundStyle(.blue)
                    Text("Dictation history").font(.system(size: 25, weight: .semibold))
                    Spacer()
                }
                Text("Saved on this Mac before paste. Copy any transcript below.")
                    .font(.system(size: 13)).foregroundStyle(.secondary)
            }
            .padding(24)
            Divider()
            if let warning = history.warning {
                Label(warning, systemImage: "exclamationmark.triangle")
                    .font(.callout).foregroundStyle(.orange).padding(16)
                Divider()
            }
            if history.entries.isEmpty {
                VStack(spacing: 14) {
                    Image(systemName: "text.bubble").font(.system(size: 38, weight: .light)).foregroundStyle(.secondary)
                    Text(history.loading ? "Loading history…" : "Your dictations will appear here")
                        .font(.system(size: 17, weight: .medium))
                    Text("Hold \(DictationShortcut.hint) to speak. Release to paste.")
                        .font(.callout).foregroundStyle(.secondary)
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(spacing: 12) {
                        ForEach(history.entries) { entry in
                            DictationRow(entry: entry, saved: !history.unsaved.contains(entry.id))
                        }
                        if history.hasMore {
                            Button(history.loading ? "Loading…" : "Load older dictations") { history.loadMore() }
                                .disabled(history.loading).padding(.vertical, 8)
                        }
                    }.padding(20)
                }
            }
            Divider()
            HStack {
                Label("\(DictationShortcut.hint) · Hold to dictate", systemImage: "keyboard")
                Spacer()
                Text("Audio is never saved")
            }.font(.system(size: 11)).foregroundStyle(.secondary).padding(.horizontal, 24).padding(.vertical, 14)
        }
        .background(.regularMaterial)
    }
}

private struct DictationRow: View {
    let entry: Dictation
    let saved: Bool
    @State private var copied = false
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 7) {
                Image(systemName: entry.source == "file" ? "doc.waveform" : "mic")
                Text(entry.createdAt.formatted(date: .abbreviated, time: .shortened))
                Text("· \(Int(entry.audioSeconds.rounded()))s")
                if !saved { Text("· Unsaved").foregroundStyle(.orange) }
                Spacer()
                Button {
                    NSPasteboard.general.clearContents()
                    copied = NSPasteboard.general.setString(entry.text, forType: .string)
                } label: {
                    Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc")
                        .frame(minWidth: 62)
                }
                .buttonStyle(.bordered)
                .accessibilityLabel(copied ? "Transcript copied" : "Copy transcript")
                .task(id: copied) {
                    if copied {
                        try? await Task.sleep(for: .seconds(2))
                        copied = false
                    }
                }
            }.font(.system(size: 11)).foregroundStyle(.secondary)
            Text(entry.text).font(.system(size: 14)).lineSpacing(4)
                .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(16)
        .background(.background.opacity(0.65), in: RoundedRectangle(cornerRadius: 16))
        .overlay(RoundedRectangle(cornerRadius: 16).strokeBorder(.primary.opacity(0.06)))
    }
}
