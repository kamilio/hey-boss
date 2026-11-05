import AppKit
import XCTest
@testable import HeyTalk

final class ClearInputTests: XCTestCase {
    @MainActor
    func testClearOnlyDeletesThePreparedField() async throws {
        guard let bundleID = ProcessInfo.processInfo.environment["HEY_TALK_CLEAR_TEST"] else { throw XCTSkip("Requires a focused scratch field") }
        let app = try XCTUnwrap(NSRunningApplication.runningApplications(withBundleIdentifier: bundleID).first)
        if NSWorkspace.shared.frontmostApplication?.processIdentifier != app.processIdentifier { app.activate() }
        var focused: FocusedInput?
        for _ in 0..<20 {
            if let input = FocusedInput.read(app: app, forAuto: false), input.isFocused() { focused = input; break }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        let field = try XCTUnwrap(focused)
        let marker = "Hey Talk clear fixture 👋"
        guard SpeechAX.string(field.element, "AXValue").trimmingCharacters(in: .newlines) == marker else {
            return XCTFail("Focus a field containing only the fixture")
        }
        let context = try XCTUnwrap(AppContext.capture(pid: app.processIdentifier))
        let request = try Correction.request(text: "Clear this field.", vocabulary: [], appContext: context, canClear: true)
        let body = try XCTUnwrap(JSONSerialization.jsonObject(with: XCTUnwrap(request.httpBody)) as? [String: Any])
        let input = try XCTUnwrap(body["input"] as? String)
        let payload = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(input.utf8)) as? [String: Any])
        XCTAssertTrue((payload["app_context"] as? String)?.contains("## Current input\n" + marker) == true)
        let correction = Correction()
        let action: CorrectionAction = try await withCheckedThrowingContinuation { continuation in
            correction.correct("Can you clear what’s in this field?", vocabulary: [], appContext: context, canClear: true) { continuation.resume(with: $0) }
        }
        guard action == .clearInput else { return XCTFail("Model did not choose clear_input") }
        guard field.isFocused(), SpeechAX.string(field.element, "AXValue").trimmingCharacters(in: .newlines) == marker else {
            return XCTFail("Scratch field changed during correction")
        }
        let board = NSPasteboard.general.changeCount
        let done = expectation(description: "cleared")
        Paste.deliver(action, target: app, originalField: field, shouldPressEnter: { XCTFail("Unexpected submit"); return true },
                      returnToField: nil, targetStillFocused: field.isFocused, onWait: nil, isCurrent: { true }) { delivery, reason in
            XCTAssertNil(reason); XCTAssertEqual(delivery, "cleared"); done.fulfill()
        }
        await fulfillment(of: [done], timeout: 2)
        try await Task.sleep(nanoseconds: 150_000_000)
        XCTAssertTrue(field.isFocused())
        XCTAssertEqual((SpeechAX.value(field.element, "AXValue") as? String)?.trimmingCharacters(in: .newlines), "")
        XCTAssertEqual(NSPasteboard.general.changeCount, board)
    }
}
