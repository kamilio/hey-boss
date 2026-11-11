import AppKit
import ScreenCaptureKit
import XCTest
@testable import HeyTalk

final class PillMediaTests: XCTestCase {
    // Set HEY_TALK_MEDIA_AUDIT to the desktop media audit folder to capture both ends of the same fixtures.
    @MainActor func testVisualsShareTheVoicePillAndSurviveTurnChanges() async throws {
        let folder = ProcessInfo.processInfo.environment["HEY_TALK_MEDIA_AUDIT"].map { URL(fileURLWithPath: $0) }
        let items: [[String: String]]
        if let folder {
            let urls = try FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)
                .filter { $0.lastPathComponent.hasPrefix("pill-") && $0.pathExtension == "json" }.sorted { $0.path < $1.path }
            items = try urls.map { try JSONDecoder().decode([String: String].self, from: Data(contentsOf: $0)) }
            XCTAssertEqual(items.count, 6)
        } else {
            let svg = #"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="1800"><rect width="300" height="100" fill="red"/></svg>"#
            items = [["id": "1", "title": "Tall diagram", "kind": "SVG", "data": Data(svg.utf8).base64EncodedString()]]
        }
        let app = NSApplication.shared, before = Set(app.windows.map(\.windowNumber))
        let service = TalkService(headless: true) { request in
            switch request["command"] as? String {
            case "conversation_media":
                XCTAssertEqual(request["sync"] as? Bool, false, "Required by the desktop request decoder")
                return items.first { $0["id"] == request["title"] as? String }
            case "conversation_control":
                XCTAssertEqual(request["title"] as? String, "hide_media")
                return ["conversation": ["active": true, "phase": "listening", "message": "Listening"]]
            default: return nil
            }
        }
        service.command(["command": "overlay", "visible": true]) { _ in }
        defer { service.command(["command": "overlay", "visible": false]) { _ in } }
        let panel = try XCTUnwrap(app.windows.first { !before.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        let root = try XCTUnwrap(panel.contentView)
        func views(_ v: NSView) -> [NSView] { [v] + v.subviews.flatMap(views) }
        func snapshot(_ name: String) async throws {
            guard let folder else { return }
            let windows = try await SCShareableContent.currentProcess.windows
            let filter = SCContentFilter(desktopIndependentWindow: try XCTUnwrap(windows.first { $0.windowID == UInt32(panel.windowNumber) }))
            let config = SCStreamConfiguration(); config.showsCursor = false; config.ignoreShadowsSingleWindow = true
            config.width = Int(panel.frame.width); config.height = Int(panel.frame.height)
            let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
            try XCTUnwrap(NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])).write(to: folder.appendingPathComponent(name + ".png"))
        }
        for (i, item) in items.enumerated() {
            panel.appearance = NSAppearance(named: i.isMultiple(of: 2) ? .aqua : .darkAqua)
            var state: [String: Any] = ["active": true, "phase": "thinking", "message": "Thinking…", "mediaID": try XCTUnwrap(item["id"])]
            service.applyConversation(state)
            state["phase"] = "listening"; state["message"] = "Listening"
            service.applyConversation(state)
            try await Task.sleep(for: .milliseconds(folder == nil ? 350 : 1100))
            let all = views(root), card = try XCTUnwrap(all.compactMap { $0 as? PillMedia }.first)
            XCTAssertTrue(views(card).contains { ($0 as? NSTextField)?.stringValue == item["title"] })
            XCTAssertTrue(all.compactMap { $0 as? NSTextField }.contains { !$0.isHidden && $0.stringValue == "Listening" })
            XCTAssertFalse(panel.canBecomeKey)
            let scroll = try XCTUnwrap(views(card).compactMap { $0 as? NSScrollView }.first)
            XCTAssertGreaterThan(scroll.contentSize.height, 80)
            XCTAssertEqual(scroll.documentVisibleRect.minY, 0, accuracy: 1)
            try await snapshot("pill-" + String(i + 1))
            if scroll.documentView!.bounds.height > scroll.contentSize.height + 1 || scroll.documentView!.bounds.width > scroll.contentSize.width + 1 {
                let fit = try XCTUnwrap(views(card).compactMap { $0 as? NSButton }.first { $0.title == "Fit" })
                fit.performClick(nil); card.layoutSubtreeIfNeeded()
                XCTAssertLessThanOrEqual(scroll.documentView!.bounds.height, scroll.contentSize.height + 1)
                XCTAssertLessThanOrEqual(scroll.documentView!.bounds.width, scroll.contentSize.width + 1)
                try await snapshot("pill-" + String(i + 1) + "-fit")
            }
        }
        let card = try XCTUnwrap(views(root).compactMap { $0 as? PillMedia }.first)
        card.close.performClick(nil)
        try await Task.sleep(for: .milliseconds(400))
        XCTAssertFalse(views(root).contains { $0 is PillMedia })
        XCTAssertTrue(views(root).compactMap { $0 as? NSTextField }.contains { !$0.isHidden && $0.stringValue == "Listening" })
    }
}
