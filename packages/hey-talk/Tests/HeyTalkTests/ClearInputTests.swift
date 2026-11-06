import AppKit
import XCTest
@testable import HeyTalk

final class ClearInputTests: XCTestCase {
    @MainActor
    func testClearThenPasteInThePreparedField() async throws {
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
        var actions: [CorrectionAction] = []
        let board = NSPasteboard.general.changeCount
        let _: CorrectionAction = try await withCheckedThrowingContinuation { continuation in
            correction.correct("Clear this field, then paste the words Check publishing.", vocabulary: [], appContext: context, canClear: true,
                               perform: { action, next in
                func fail(_ message: String) {
                    continuation.resume(throwing: NSError(domain: "ClearInputTests", code: 1, userInfo: [NSLocalizedDescriptionKey: message]))
                }
                switch (actions.count, action) {
                case (0, .clearInput): break
                case (1, .paste(let text)) where text.trimmingCharacters(in: .punctuationCharacters).lowercased() == "check publishing": break
                default: fail("Unexpected tool at step \(actions.count + 1): \(action)"); return
                }
                guard field.isFocused(),
                      SpeechAX.string(field.element, "AXValue").trimmingCharacters(in: .newlines) == (actions.isEmpty ? marker : "")
                else { fail("Scratch input lost focus or changed before tool \(actions.count + 1)"); return }
                actions.append(action)
                Paste.deliver(action, target: app, originalField: field, shouldPressEnter: { false },
                              returnToField: nil, needsField: true, onWait: nil, isCurrent: { true }) { delivery, reason in
                    if let reason { fail("Delivery blocked: " + reason.rawValue); return }
                    if action == .clearInput { XCTAssertEqual(NSPasteboard.general.changeCount, board) }
                    next(delivery)
                }
            }) { continuation.resume(with: $0) }
        }
        XCTAssertEqual(actions.count, 2)
        XCTAssertTrue(field.isFocused())
        XCTAssertEqual(SpeechAX.string(field.element, "AXValue").trimmingCharacters(in: .newlines), actions.last?.text)
    }
}
