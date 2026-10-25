import AppKit
import SwiftUI

final class ContextWindow {
    private let window: NativeWindow
    private var source: NSRunningApplication?
    var isVisible: Bool { window.isVisible }

    init() {
        window = NativeWindow(contentRect: NSRect(x: 0, y: 0, width: 760, height: 580),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                              backing: .buffered, defer: false)
        window.minSize = NSSize(width: 480, height: 360)
        window.center()
    }
    func show(source: NSRunningApplication, text: String?, milliseconds: Double) {
        self.source = source
        let name = source.localizedName ?? "Current app"
        window.title = "Hey Talk — Context from \(name)"
        window.host(ContextView(app: name, text: text, milliseconds: milliseconds))
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }
    func close() {
        window.close()
        source?.activate(options: [])
    }
}

private struct ContextView: View {
    let app: String
    let text: String?
    let milliseconds: Double

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 14) {
                Image(systemName: "text.viewfinder")
                    .font(.system(size: 24, weight: .medium)).foregroundStyle(.teal)
                    .frame(width: 52, height: 52)
                    .background(.teal.opacity(0.10), in: RoundedRectangle(cornerRadius: 16))
                VStack(alignment: .leading, spacing: 5) {
                    Text("Context preview").font(.system(size: 24, weight: .semibold, design: .rounded))
                    Text("\(app) · \(text?.count ?? 0) characters · \(Int(milliseconds.rounded())) ms")
                        .font(.system(size: 12)).foregroundStyle(.secondary)
                }
                Spacer()
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(text ?? "", forType: .string)
                } label: { Label("Copy", systemImage: "doc.on.doc") }
                    .disabled(text == nil)
            }
            .padding(.horizontal, 26).padding(.top, 38).padding(.bottom, 20)
            Divider().padding(.horizontal, 26)
            if let text {
                ScrollView {
                    Text(verbatim: text)
                        .font(.system(size: 13, design: .monospaced)).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(20)
                }
                .background(.primary.opacity(0.035), in: RoundedRectangle(cornerRadius: 12))
                .padding(.horizontal, 26).padding(.top, 18)
            } else {
                VStack(spacing: 12) {
                    Image(systemName: "text.magnifyingglass").font(.system(size: 32, weight: .light)).foregroundStyle(.secondary)
                    Text("No visible context captured").font(.headline)
                    Text(AXIsProcessTrusted() ? "This window isn’t exposing readable text through Accessibility." : "Allow Hey Talk in System Settings → Privacy & Security → Accessibility.")
                        .font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
                }.frame(maxWidth: .infinity, maxHeight: .infinity).padding(26)
            }
            HStack {
                Text("Local snapshot · the same capture used for spelling correction")
                Spacer()
                Text("Hyper+M").fontWeight(.medium)
            }
            .font(.system(size: 11)).foregroundStyle(.secondary).padding(.horizontal, 26).padding(.vertical, 18)
        }
    }
}
