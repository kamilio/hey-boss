import AppKit
import TalkCore
import XCTest
@testable import HeyTalk

final class T3ToolsTests: XCTestCase {
    @MainActor
    func testLiveThreadSwitchKeepsVoiceConversation() async throws {
        guard ProcessInfo.processInfo.environment["HEY_TALK_T3_VOICE_TEST"] == "1" else { throw XCTSkip("Requires a running voice conversation and a focused T3 composer") }
        func checkVoice() throws {
            let status = try ControlSocket.request(["command": "status"])
            let chat = try XCTUnwrap(status["conversation"] as? [String: Any])
            XCTAssertEqual(chat["microphone"] as? Bool, true)
            XCTAssertEqual(status["state"] as? String, "idle", "Dictation must not take over the new composer")
            XCTAssertEqual(status["escape_registered"] as? Bool, false, "T3 must own its navigation keys")
        }
        try checkVoice()
        let app = try XCTUnwrap(NSRunningApplication.runningApplications(withBundleIdentifier: "com.t3tools.t3code").first)
        app.activate(options: [])
        try await Task.sleep(for: .milliseconds(200))
        let field = try XCTUnwrap(FocusedInput.read(app: app, forAuto: false))
        let original = try XCTUnwrap(field.document.flatMap(FocusedInput.threadLink))
        defer { NSWorkspace.shared.open(original) }
        let next = await field.nextThread(isCurrent: { true })
        XCTAssertNotEqual(try XCTUnwrap(next).document, field.document)
        try await Task.sleep(for: .seconds(2.2)) // Past dictation’s focus-loss grace period.
        try checkVoice()
    }

    @MainActor
    func testLiveNavigationPreservesDraftAndClipboard() async throws {
        guard ProcessInfo.processInfo.environment["HEY_TALK_T3_TOOLS_TEST"] == "1" else { throw XCTSkip("Requires a focused T3 thread with Auto off") }
        let app = try XCTUnwrap(NSRunningApplication.runningApplications(withBundleIdentifier: "com.t3tools.t3code").first)
        app.activate(options: [])
        try await Task.sleep(nanoseconds: 150_000_000)
        let field = try XCTUnwrap(FocusedInput.read(app: app, forAuto: false))
        let draft = SpeechAX.string(field.element, "AXValue")
        let clipboard = NSPasteboard.general.changeCount
        let rows = try XCTUnwrap(field.threadRows())
        let original = try XCTUnwrap(rows.firstIndex(where: { $0.current }))
        XCTAssertEqual(rows[original].title, field.composerLabel.map { String($0.dropFirst("Message ".count)) })
        XCTAssertEqual(rows.count, 9)

        let stale = rows.map { T3ThreadRow(title: "stale", label: $0.label, element: $0.element) }
        let rejected = await field.switchThread(position: 1, listed: stale, isCurrent: { true })
        XCTAssertNil(rejected)
        XCTAssertTrue(field.isFocused())
        let destination = original == 0 ? 1 : 0
        let selected = await field.switchThread(position: destination + 1, listed: rows, isCurrent: { true })
        let moved = try XCTUnwrap(selected)
        XCTAssertEqual(moved.composerLabel, "Message " + rows[destination].title)
        let next = await moved.nextThread(isCurrent: { true })
        XCTAssertNotNil(next, "T3’s native next-chat shortcut must focus a different chat")
        XCTAssertNotEqual(next?.t3ThreadID, moved.t3ThreadID)
        let returned = await (next ?? moved).switchThread(position: original + 1, listed: nil, isCurrent: { true })
        let restored = try XCTUnwrap(returned)
        XCTAssertEqual(restored.document, field.document)

        XCTAssertEqual(SpeechAX.string(restored.element, "AXValue"), draft)
        XCTAssertEqual(NSPasteboard.general.changeCount, clipboard)
        XCTAssertTrue(restored.isFocused())
    }

    func testDesiredServerStateIsIdempotent() throws {
        let id = UUID().uuidString
        let unpinned: [String: Any] = ["id": id, "pinnedAt": NSNull(), "title": "Before", "settledOverride": NSNull()]
        var pinned = unpinned; pinned["pinnedAt"] = "2026-11-04T00:00:00Z"
        XCTAssertEqual(try T3Server.command(.pinThread(true), thread: unpinned)?["type"] as? String, "thread.pin")
        XCTAssertNil(try T3Server.command(.pinThread(true), thread: pinned))
        XCTAssertEqual(try T3Server.command(.pinThread(false), thread: pinned)?["type"] as? String, "thread.unpin")
        XCTAssertNil(try T3Server.command(.pinThread(false), thread: unpinned))
        XCTAssertEqual(try T3Server.command(.settleThread, thread: unpinned)?["type"] as? String, "thread.settle")
        var settled = unpinned; settled["settledOverride"] = "settled"
        XCTAssertNil(try T3Server.command(.settleThread, thread: settled))
        XCTAssertNil(try T3Server.command(.rename("Before"), thread: unpinned))
        XCTAssertEqual(try T3Server.command(.rename("After"), thread: unpinned)?["title"] as? String, "After")
        XCTAssertThrowsError(try T3Server.command(.pinThread(true), thread: ["id": "invalid"]))
        XCTAssertThrowsError(try T3Server.command(.paste("oops"), thread: unpinned))
    }

    func testLiveRemoteCatchUp() async throws {
        guard let environment = ProcessInfo.processInfo.environment["HEY_TALK_T3_ENVIRONMENT"],
              let id = ProcessInfo.processInfo.environment["HEY_TALK_T3_SERVER_THREAD"] else { throw XCTSkip("Requires a connected T3 thread with a reply") }
        let recap = try await T3Server.shared.catchUp(id, environment: environment)
        XCTAssertFalse(recap.replies?.isEmpty ?? true)
    }

    func testLiveServerPinUnpinAndSettleState() async throws {
        guard let id = ProcessInfo.processInfo.environment["HEY_TALK_T3_SERVER_THREAD"] else { throw XCTSkip("Requires a local T3 thread ID") }
        let environment = try ProcessInfo.processInfo.environment["HEY_TALK_T3_ENVIRONMENT"] ?? String(contentsOf: FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".t3/userdata/environment-id"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
        let server = T3Server.shared
        let original = try await server.thread(id, environment: environment)
        let title = try XCTUnwrap(original["title"] as? String)
        let pinned = original["pinnedAt"] as? String != nil
        do {
            for desired in [!pinned, !pinned, pinned, pinned] {
                try await server.apply(.pinThread(desired), thread: id, environment: environment, isCurrent: { true })
                let state = try await server.thread(id, environment: environment)
                XCTAssertEqual(state["pinnedAt"] as? String != nil, desired)
            }
            let threads = try await server.threads(environment: environment)
            let settled = try XCTUnwrap(threads.first { $0["settledOverride"] as? String == "settled" && $0["pinnedAt"] as? String == nil })
            try await server.apply(.settleThread, thread: try XCTUnwrap(settled["id"] as? String), environment: environment, isCurrent: { true })
            try await server.apply(.rename(title + " test"), thread: id, environment: environment, isCurrent: { true })
            try await server.apply(.rename(title), thread: id, environment: environment, isCurrent: { true })
        } catch {
            try await server.apply(.pinThread(pinned), thread: id, environment: environment, isCurrent: { true })
            try await server.apply(.rename(title), thread: id, environment: environment, isCurrent: { true })
            throw error
        }
    }
}
