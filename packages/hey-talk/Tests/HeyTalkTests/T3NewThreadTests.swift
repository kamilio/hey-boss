import AppKit
import XCTest
@testable import HeyTalk

final class T3NewThreadTests: XCTestCase {
    @MainActor
    func testLiveNewThreadLeavesDraftAndClipboardAlone() async throws {
        guard ProcessInfo.processInfo.environment["HEY_TALK_T3_NEW_TEST"] == "1" else { throw XCTSkip("Requires a focused T3 thread") }
        let field = try XCTUnwrap(FocusedInput.current(bundleIDs: ["com.t3tools.t3code"]))
        let oldLink = try XCTUnwrap(field.document.flatMap(FocusedInput.threadLink))
        defer { NSWorkspace.shared.open(oldLink) }
        let project = ProcessInfo.processInfo.environment["HEY_TALK_T3_PROJECT"]
        let expectedProject = try XCTUnwrap(project ?? field.projectButton?.name)
        let speech = project.map { "New thread " + $0 } ?? "New thread"
        let correction = Correction()
        let action: CorrectionAction = try await withCheckedThrowingContinuation { continuation in
            correction.correct(speech, vocabulary: [], appContext: "New thread in another project.", threadActions: true) { continuation.resume(with: $0) }
        }
        let board = NSPasteboard.general.changeCount
        let done = expectation(description: "opened")
        Paste.deliver(action, target: field.app, originalField: field, shouldPressEnter: { XCTFail("Unexpected submit"); return true },
                      returnToField: nil, targetStillFocused: field.isFocused, onWait: nil, isCurrent: { true }) { delivery, reason in
            XCTAssertEqual(delivery, "new_thread"); XCTAssertNil(reason); done.fulfill()
        }
        await fulfillment(of: [done], timeout: 5)
        for _ in 0..<40 {
            try await Task.sleep(nanoseconds: 50_000_000)
            if let fresh = FocusedInput.current(bundleIDs: ["com.t3tools.t3code"]), fresh.document?.hasPrefix("t3code://app/#/draft/") == true,
               fresh.composerLabel != nil {
                XCTAssertEqual(SpeechAX.string(fresh.element, "AXValue").trimmingCharacters(in: .whitespacesAndNewlines), "")
                XCTAssertEqual(fresh.projectButton?.name, expectedProject)
                XCTAssertEqual(NSPasteboard.general.changeCount, board)
                return
            }
        }
        XCTFail("A fresh composer never appeared")
    }
}
