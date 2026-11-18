import AppKit
import SwiftUI

final class PickerPanel: NSPanel, NSWindowDelegate {
    private let model: ClipboardApp
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }
    init(model: ClipboardApp) {
        self.model = model
        super.init(contentRect: NSRect(x: 0, y: 0, width: 660, height: 640), styleMask: [.borderless], backing: .buffered, defer: false)
        title = "Hey Clipboard"
        isOpaque = false; backgroundColor = .clear; hasShadow = true
        level = .normal; isReleasedWhenClosed = false; delegate = self
        collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
        let host = NSHostingView(rootView: PickerView(model: model).clipShape(RoundedRectangle(cornerRadius: 24)))
        host.sizingOptions = []; host.wantsLayer = true; host.layer?.backgroundColor = NSColor.clear.cgColor
        // Clip the entire backing surface, not only the glass decoration. Otherwise
        // the hosting view leaves square corners underneath the rounded material.
        let surface = NSView(frame: NSRect(origin: .zero, size: frame.size))
        surface.wantsLayer = true
        surface.layer?.cornerRadius = 24
        surface.layer?.masksToBounds = true
        let material: NSView
        if #available(macOS 26, *) {
            let glass = NSGlassEffectView(); glass.style = .regular; glass.cornerRadius = 24
            glass.contentView = host; material = glass
        } else {
            let glass = NSVisualEffectView(); glass.material = .popover; glass.blendingMode = .behindWindow; glass.state = .active
            glass.wantsLayer = true; glass.layer?.cornerRadius = 24; glass.layer?.masksToBounds = true
            host.frame = glass.bounds; host.autoresizingMask = [.width, .height]; glass.addSubview(host); material = glass
        }
        material.frame = surface.bounds
        material.autoresizingMask = [.width, .height]
        surface.addSubview(material)
        contentView = surface
    }
    func centerOnMouseScreen() {
        let screen = NSScreen.screens.first { $0.frame.contains(NSEvent.mouseLocation) } ?? NSScreen.main
        guard let frame = screen?.visibleFrame else { return }
        setFrameOrigin(NSPoint(x: frame.midX - self.frame.width / 2, y: frame.midY - self.frame.height / 2 + 45))
    }
    func windowDidResignKey(_ notification: Notification) { model.cycling = false }
    override func sendEvent(_ event: NSEvent) {
        if event.type == .flagsChanged { model.flagsChanged(event.modifierFlags) }
        if event.type == .keyDown {
            // Navigation, confirmation, and cancellation also work while the
            // shortcut's Command and Shift modifiers remain held.
            switch event.keyCode {
            case 53: model.dismiss(); return
            case 125: model.move(1); return
            case 126: model.move(-1); return
            case 36, 76: model.pasteSelection(); return
            default: break
            }
            let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
            if modifiers == .command, event.keyCode == 51 { model.deleteSelection(); return }
        }
        super.sendEvent(event)
    }
}

private struct PickerView: View {
    @ObservedObject var model: ClipboardApp

    var body: some View {
        if model.browsing { HistoryView(model: model, history: model.history) }
        else { QuickPickerView(model: model, history: model.history) }
    }
}

private struct QuickPickerView: View {
    @ObservedObject var model: ClipboardApp
    @ObservedObject var history: History

    var body: some View {
        VStack(spacing: 0) {
            if history.clips.isEmpty {
                Text("Copy something to get started").foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(spacing: 3) {
                            ForEach(history.clips) { clip in
                                Text(clip.text).font(.system(size: 14)).lineLimit(2)
                                    .frame(maxWidth: .infinity, alignment: .leading).padding(12)
                                    .background(.teal.opacity(model.selection?.id == clip.id ? 0.16 : 0), in: RoundedRectangle(cornerRadius: 12))
                                    .contentShape(Rectangle())
                                    .onTapGesture(count: 2) { model.selected = clip.id; model.pasteSelection() }
                                    .onTapGesture { model.selected = clip.id }
                                    .id(clip.id)
                            }
                        }.padding(10)
                    }
                    .onChange(of: model.selected) { _, id in if let id { proxy.scrollTo(id) } }
                }
            }
            if let warning = model.message {
                Text(warning).font(.system(size: 11)).foregroundStyle(.secondary).padding(.horizontal, 18).padding(.bottom, 8)
            }
            HStack {
                Text("\((history.clips.firstIndex { $0.id == model.selection?.id }).map { $0 + 1 } ?? 0) / \(history.clips.count)")
                Spacer()
                Text("↑↓   ↵ Paste   esc")
            }.font(.system(size: 11)).foregroundStyle(.secondary).padding(.horizontal, 18).padding(.bottom, 12)
        }
    }
}

private struct HistoryView: View {
    @ObservedObject var model: ClipboardApp
    @ObservedObject var history: History
    @FocusState private var searchFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                HStack(spacing: 9) {
                    Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                    TextField("Search clipboard history", text: $model.query)
                        .textFieldStyle(.plain).focused($searchFocused).accessibilityLabel("Search clipboard history")
                    if !model.query.isEmpty {
                        Button { model.query = "" } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary) }
                            .buttonStyle(.plain).accessibilityLabel("Clear search")
                    }
                }.padding(12).background(.primary.opacity(0.045), in: RoundedRectangle(cornerRadius: 12))
                Button { model.pinsOnly.toggle() } label: {
                    Image(systemName: model.pinsOnly ? "pin.fill" : "pin").foregroundStyle(model.pinsOnly ? Color.teal : .secondary)
                        .frame(width: 38, height: 38)
                }.buttonStyle(.plain).background(.primary.opacity(0.045), in: RoundedRectangle(cornerRadius: 12))
                    .help("Show pinned entries").accessibilityLabel("Show pinned entries")
                Button { model.togglePause() } label: {
                    Image(systemName: model.paused ? "play.fill" : "pause.fill")
                }.buttonStyle(.plain).foregroundStyle(.secondary)
                    .accessibilityLabel(model.paused ? "Resume capture" : "Pause capture")
                Button { model.dismiss() } label: { Image(systemName: "xmark") }
                    .buttonStyle(.plain).foregroundStyle(.secondary).padding(.leading, 8).accessibilityLabel("Close")
            }.padding(.horizontal, 24).padding(.top, 20).padding(.bottom, 12)
            if let warning = history.warning ?? model.message {
                Text(warning).font(.system(size: 12)).foregroundStyle(.secondary).padding(.horizontal, 24).padding(.bottom, 8)
            }
            if model.visible.isEmpty {
                VStack(spacing: 12) {
                    Image(systemName: model.query.isEmpty ? "clipboard" : "magnifyingglass").font(.system(size: 32, weight: .light)).foregroundStyle(.teal)
                    Text(model.query.isEmpty ? "Copy something to get started" : "No matching clips").font(.system(size: 16, weight: .medium))
                    Text(model.query.isEmpty ? "Your last 40 clips stay on this Mac. Pin the ones you want to keep." : "Try a word from the text or the app you copied it from.")
                        .font(.system(size: 12)).foregroundStyle(.secondary).multilineTextAlignment(.center).frame(maxWidth: 330)
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(spacing: 3) {
                            ForEach(model.visible) { clip in
                                HStack(alignment: .top, spacing: 12) {
                                    Image(systemName: clip.pinned ? "pin.fill" : "doc.on.clipboard")
                                        .font(.system(size: 13)).foregroundStyle(.teal).frame(width: 20).padding(.top, 3)
                                    VStack(alignment: .leading, spacing: 6) {
                                        Text(clip.text).font(.system(size: 13)).lineLimit(2).frame(maxWidth: .infinity, alignment: .leading)
                                        HStack(spacing: 6) {
                                            Text(clip.app)
                                            Text("·")
                                            Text(clip.copiedAt, style: .relative)
                                        }.font(.system(size: 10)).foregroundStyle(.secondary)
                                    }
                                    if model.selection?.id == clip.id {
                                        Image(systemName: "return").font(.system(size: 12)).foregroundStyle(.teal).padding(.top, 3)
                                    }
                                }
                                .padding(13).frame(maxWidth: .infinity, alignment: .leading)
                                .background(.teal.opacity(model.selection?.id == clip.id ? 0.13 : 0), in: RoundedRectangle(cornerRadius: 13))
                                .contentShape(Rectangle())
                                .onTapGesture(count: 2) { model.selected = clip.id; model.pasteSelection() }
                                .onTapGesture { model.selected = clip.id }
                                .contextMenu {
                                    Button("Copy") { model.selected = clip.id; model.copySelection() }
                                    Button(clip.pinned ? "Unpin" : "Pin") { history.pin(clip.id, !clip.pinned) }
                                    Divider()
                                    Button("Delete", role: .destructive) { history.delete(clip.id) }
                                }
                                .id(clip.id)
                            }
                        }.padding(.horizontal, 14).padding(.vertical, 2)
                    }
                    .onChange(of: model.selected) { _, id in if let id { proxy.scrollTo(id) } }
                }
                Divider().padding(.horizontal, 24).padding(.top, 8)
                if let clip = model.selection {
                    ScrollView {
                        Text(clip.text).font(.system(size: 12)).textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 24).padding(.vertical, 12)
                    }.frame(height: 110)
                }
            }
            Divider().padding(.horizontal, 24)
            HStack(spacing: 12) {
                Label(model.cycling ? "Release ⌘⇧ to paste" : model.paused ? "Capture paused" : "On this Mac", systemImage: model.cycling ? "return" : model.paused ? "pause.circle" : "lock.shield")
                    .foregroundStyle(.secondary)
                Spacer()
                if let clip = model.selection {
                    Button { history.pin(clip.id, !clip.pinned) } label: { Image(systemName: clip.pinned ? "pin.slash" : "pin") }
                        .buttonStyle(.plain).help(clip.pinned ? "Unpin" : "Pin").accessibilityLabel(clip.pinned ? "Unpin" : "Pin")
                    Button("Copy") { model.copySelection() }.buttonStyle(.plain)
                    Button { model.pasteSelection() } label: { Label("Paste", systemImage: "return").padding(.horizontal, 9).padding(.vertical, 6) }
                        .buttonStyle(.plain).foregroundStyle(.teal).background(.teal.opacity(0.12), in: Capsule())
                }
            }.font(.system(size: 11, weight: .medium)).padding(.horizontal, 24).padding(.vertical, 14)
        }
        .onAppear { searchFocused = true }
        .onReceive(model.$browsing) { if $0 { searchFocused = true } }
        .onChange(of: model.query) { _, _ in model.selected = model.visible.first?.id }
        .onChange(of: model.pinsOnly) { _, _ in model.selected = model.visible.first?.id }
    }
}
