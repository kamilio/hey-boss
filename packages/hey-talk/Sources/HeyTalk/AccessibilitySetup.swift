import AppKit
import SwiftUI

/// A guide only: macOS owns the permission switch and the user must enable it.
final class AccessibilitySetup: NSObject, ObservableObject, NSWindowDelegate {
    @Published private(set) var granted = AXIsProcessTrusted()
    private var window: NSWindow?
    private var timer: Timer?
    private static let settingsURL = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")!

    func show() {
        refresh()
        if window == nil {
            let panel = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 540, height: 540),
                                 styleMask: [.titled, .closable], backing: .buffered, defer: false)
            panel.title = "Hey Talk — Automatic Paste"
            panel.isReleasedWhenClosed = false
            panel.delegate = self
            panel.contentView = NSHostingView(rootView: AccessibilitySetupView(setup: self))
            panel.center(); window = panel
        }
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        timer?.invalidate()
        let ticker = Timer(timeInterval: 1, repeats: true) { [weak self] _ in self?.refresh() }
        ticker.tolerance = 0.2
        RunLoop.main.add(ticker, forMode: .common); timer = ticker
    }

    func openSettings() {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
        _ = AXIsProcessTrustedWithOptions(options)
        NSWorkspace.shared.open(Self.settingsURL)
    }

    func revealApp() {
        NSWorkspace.shared.activateFileViewerSelecting([Bundle.main.bundleURL])
    }

    private func refresh() { granted = AXIsProcessTrusted() }
    func windowWillClose(_ notification: Notification) { timer?.invalidate(); timer = nil }
    deinit { timer?.invalidate() }
}

private struct AccessibilitySetupView: View {
    @ObservedObject var setup: AccessibilitySetup
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack(spacing: 14) {
                Image(systemName: setup.granted ? "checkmark.circle.fill" : "text.cursor")
                    .font(.system(size: 34)).foregroundStyle(setup.granted ? .green : .blue)
                VStack(alignment: .leading, spacing: 5) {
                    Text(setup.granted ? "Automatic paste is ready" : "Enable automatic paste")
                        .font(.system(size: 23, weight: .semibold))
                    Text(setup.granted ? "Your next dictation can paste into the active app." : "One macOS permission, then you’re set.")
                        .font(.callout).foregroundStyle(.secondary)
                }
            }
            if setup.granted {
                Text("Press Caps Lock + R to start recording. Press again to finish. Every transcript is also saved in Dictation History.")
                    .font(.body).lineSpacing(4)
                Spacer()
            } else {
                Text("Accessibility lets Hey Talk press ⌘V in the app where you started dictating.")
                    .font(.body).lineSpacing(4)
                VStack(alignment: .leading, spacing: 14) {
                    step("1", "Open Accessibility settings below.")
                    step("2", "Turn on Hey Talk in the app list.")
                    step("3", "Missing from the list? Click +, press ⇧⌘G, enter ~/Applications, then choose Hey Talk.app.")
                }
                Text("You can also drag Hey Talk from Finder into that list. This window updates when access is enabled.")
                    .font(.callout).foregroundStyle(.secondary)
                Spacer(minLength: 0)
                HStack {
                    Button("Show Hey Talk in Finder") { setup.revealApp() }
                    Spacer()
                    Button("Open Accessibility Settings") { setup.openSettings() }
                        .buttonStyle(.borderedProminent)
                }
            }
        }
        .padding(28)
        .frame(width: 540, height: 540)
        .background(.regularMaterial)
    }
    private func step(_ number: String, _ text: String) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Text(number).font(.system(size: 12, weight: .semibold))
                .frame(width: 22, height: 22)
                .background(.blue.opacity(0.12), in: Circle())
            Text(text).font(.system(size: 13)).fixedSize(horizontal: false, vertical: true)
        }
    }
}
