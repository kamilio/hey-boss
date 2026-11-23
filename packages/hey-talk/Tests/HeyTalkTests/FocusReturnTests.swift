import AppKit
import XCTest
@testable import HeyTalk

final class FocusReturnTests: XCTestCase {
    @MainActor
    private func liveField() async throws -> FocusedInput {
        guard ProcessInfo.processInfo.environment["HEY_TALK_FOCUS_TEST"] == "1" else { throw XCTSkip("Requires T3 with a focused composer and Accessibility") }
        _ = NSApplication.shared
        if NSApp.activationPolicy() != .accessory { NSApp.setActivationPolicy(.accessory) }
        let app = try XCTUnwrap(NSRunningApplication.runningApplications(withBundleIdentifier: "com.t3tools.t3code").first)
        app.activate()
        for _ in 0..<20 {
            if app.isActive { break }
            try await Task.sleep(for: .milliseconds(100))
        }
        let window = try XCTUnwrap(ConversationContext.focusedWindow(app))
        let page = try XCTUnwrap(BrowserPage.page(in: window, deadline: ProcessInfo.processInfo.systemUptime + 1))
        let query = ["AXSearchKey": "AXTextFieldSearchKey", "AXResultsLimit": 64] as CFDictionary
        let editors = (SpeechAX.parameter(page.area, "AXUIElementsForSearchPredicate", query) as? [AXUIElement] ?? [])
            .compactMap { FocusedInput.identify($0, app: app) }.filter { $0.composerLabel != nil }
        XCTAssertEqual(editors.count, 1)
        let field = try XCTUnwrap(editors.first)
        for _ in 0..<20 {
            field.restoreFocus()
            try await Task.sleep(for: .milliseconds(200))
            if field.isFocused(), app.isActive {
                try await Task.sleep(for: .milliseconds(200))
                if field.isFocused(), app.isActive { break }
            }
        }
        XCTAssertTrue(field.isFocused())
        XCTAssertTrue(app.isActive)
        return field
    }

    @MainActor
    func testLiveReturnToOriginalThread() async throws {
        let field = try await liveField(), app = field.app
        let rows = try XCTUnwrap(field.threadRows())
        let original = try XCTUnwrap(rows.first(where: { $0.current }))
        let other = try XCTUnwrap(rows.first(where: { !$0.current }))
        let clipboard = NSPasteboard.general.changeCount
        let draft = SpeechAX.string(field.element, "AXValue")
        defer {
            AXUIElementPerformAction(original.element, "AXPress" as CFString)
            field.restoreFocus()
        }
        XCTAssertEqual(AXUIElementPerformAction(other.element, "AXPress" as CFString), .success)
        try await Task.sleep(for: .milliseconds(500))
        XCTAssertNotEqual(FocusedInput.read(app: app, forAuto: false)?.document, field.document)
        field.restoreFocus()
        for _ in 0..<30 {
            if field.isFocused() { break }
            try await Task.sleep(for: .milliseconds(100))
            field.restoreFocus()
        }
        XCTAssertTrue(field.isFocused(), "Return must navigate back to the original thread")
        XCTAssertEqual(NSPasteboard.general.changeCount, clipboard)
        if let fresh = FocusedInput.read(app: app, forAuto: false), fresh == field {
            XCTAssertEqual(SpeechAX.string(fresh.element, "AXValue"), draft)
        }
    }

    @MainActor
    func testLiveReturnFromNonactivatingPanel() async throws {
        let field = try await liveField(), app = field.app
        XCTAssertTrue(field.isFocused())
        let clipboard = NSPasteboard.general.changeCount
        let draft = SpeechAX.string(field.element, "AXValue")
        let panel = NSPanel(contentRect: NSRect(x: 200, y: 200, width: 240, height: 100),
                            styleMask: [.titled, .nonactivatingPanel], backing: .buffered, defer: false)
        let editor = NSTextView(frame: panel.contentView!.bounds)
        panel.contentView = editor
        panel.makeKeyAndOrderFront(nil)
        panel.makeFirstResponder(editor)
        defer { panel.orderOut(nil) }
        try await Task.sleep(for: .milliseconds(200))
        XCTAssertTrue(app.isActive, "A nonactivating panel leaves T3 active")
        XCTAssertNotEqual(FocusedInput.keyboardApp, app)
        XCTAssertFalse(field.isFocused())
        field.restoreFocus()
        for _ in 0..<30 {
            if field.isFocused() { break }
            try await Task.sleep(for: .milliseconds(100))
            field.restoreFocus()
        }
        XCTAssertTrue(field.isFocused(), "Return must reclaim keyboard focus from the panel")
        XCTAssertEqual(NSPasteboard.general.changeCount, clipboard)
        XCTAssertEqual(SpeechAX.string(field.element, "AXValue"), draft)
    }

    @MainActor
    func testLivePendingReturnAfterTwoSecondsAndManualMode() async throws {
        for automatic in [true, false] {
            let field = try await liveField()
            let panel = NSPanel(contentRect: NSRect(x: 200, y: 200, width: 240, height: 100),
                                styleMask: [.titled, .nonactivatingPanel], backing: .buffered, defer: false)
            let editor = NSTextView(frame: panel.contentView!.bounds)
            panel.contentView = editor
            panel.makeKeyAndOrderFront(nil); panel.makeFirstResponder(editor)
            defer { panel.orderOut(nil) }
            try await Task.sleep(for: .milliseconds(200))
            XCTAssertFalse(field.isFocused())
            var retry: Timer?, current = true, completed = false
            defer { retry?.invalidate() }
            // listThreads completes without posting keys or invoking a server action.
            Paste.deliver(.listThreads, target: field.app, originalField: field, shouldPressEnter: { false },
                          returnToField: automatic ? field : nil, needsField: true,
                          onWait: { retry = $0; $0.fireDate = .distantFuture }, isCurrent: { current }) { _, _ in completed = true }
            let timer = try XCTUnwrap(retry)
            // Simulate a busy app/run loop, beyond the old restoration deadline.
            try await Task.sleep(for: .seconds(2.2))
            for _ in 0..<20 {
                timer.fire()
                try await Task.sleep(for: .milliseconds(100))
                if completed || !automatic { break }
            }
            XCTAssertEqual(completed, automatic)
            XCTAssertEqual(field.isFocused(), automatic)
            current = false
            timer.fire()
            XCTAssertFalse(timer.isValid, "Cancellation must stop restoration")
        }
    }
}
