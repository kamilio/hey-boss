// A separate Dock entry for the chat hosted by the signed desktop app.
import AppKit

final class ChatsDock: NSObject, NSApplicationDelegate {
    private var opening = false

    func applicationDidBecomeActive(_ notification: Notification) { openChat() }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        openChat(); return false
    }
    private func openChat() {
        guard !opening else { return }
        opening = true
        if let desktop = NSRunningApplication.runningApplications(withBundleIdentifier: "local.hey-boss.desktop").first {
            NSApp.yieldActivation(to: desktop)
        }
        DispatchQueue.global(qos: .userInitiated).async {
            let opened = Self.requestChat()
            DispatchQueue.main.async {
                if opened { self.opening = false }
                else {
                    let alert = NSAlert(); alert.messageText = "Couldn’t open Chats"
                    alert.informativeText = "Check that Hey Boss is installed and running on this Mac."
                    alert.runModal(); NSApp.terminate(nil)
                }
            }
        }
    }
    private static func requestChat() -> Bool {
        let plist = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/LaunchAgents/local.hey-boss.plist")
        let sockets = NSDictionary(contentsOf: plist)?["Sockets"] as? [String: [String:Any]]
        var address = sockaddr_un()
        guard let path = sockets?["Listener"]?["SockPathName"] as? String,
              path.utf8.count < MemoryLayout.size(ofValue: address.sun_path) else { return false }
        address.sun_family = sa_family_t(AF_UNIX); address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        withUnsafeMutableBytes(of: &address.sun_path) { bytes in path.utf8CString.withUnsafeBytes { bytes.copyMemory(from: $0) } }
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return false }
        let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        var timeout = timeval(tv_sec: 5, tv_usec: 0), yes: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout.size(ofValue: timeout)))
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &yes, socklen_t(MemoryLayout.size(ofValue: yes)))
        guard withUnsafePointer(to: &address, { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
        } }) == 0 else { return false }
        do {
            try handle.write(contentsOf: Data(#"{"command":"conversation_control","title":"show","sync":false}"#.utf8))
            shutdown(fd, SHUT_WR)
            guard let data = try handle.readToEnd() else { return false }
            return (try JSONSerialization.jsonObject(with: data) as? [String:Any])?["status"] as? String == "ok"
        } catch { return false }
    }
}

let app = NSApplication.shared, delegate = ChatsDock()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
