import AppKit
import XCTest
@testable import HeyTalk

final class PasteTests: XCTestCase {
    func testClosingPendingPasteCopiesExactTranscriptAndStopsRetrying() {
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        let text = String(repeating: "A long dictation…\n", count: 100)
        let retry = Timer(timeInterval: 0.35, repeats: true) { _ in XCTFail("Dismissed paste retried") }
        RunLoop.main.add(retry, forMode: .common)
        XCTAssertTrue(Paste.copyAndStopWaiting(text, retry: retry, to: board))
        XCTAssertEqual(board.string(forType: .string), text)
        XCTAssertFalse(retry.isValid)
    }

    func testRecoveryNoticeNamesOriginalAppAndCorrectAction() {
        XCTAssertEqual(PasteBlockReason.focusChanged.message(targetName: "Slack"), "Return to Slack. Press ⌘V.")
        XCTAssertEqual(PasteBlockReason.targetClosed.message(targetName: "Notes"), "Reopen Notes. Press ⌘V.")
        XCTAssertEqual(PasteBlockReason.focusChanged.message(targetName: "Slack", pasted: true), "Pasted in Slack. Focus changed before submit.")
        XCTAssertEqual(PasteBlockReason.eventUnavailable.message(targetName: "cmux", pasted: true), "Pasted in cmux. Couldn’t send Enter.")
        XCTAssertEqual(PasteBlockReason.focusChanged.message(targetName: nil), "Return to the original app. Press ⌘V.")
        XCTAssertEqual(PasteBlockReason.focusChanged.message(targetName: "  "), "Return to the original app. Press ⌘V.")
    }

    func testPasteOnlyNeverSchedulesEnter() {
        var keys: [CGKeyCode] = []
        Paste.send(shouldPressEnter: { false }, check: { nil }, isCurrent: { true }, post: { key, _ in keys.append(key); return true }, afterPaste: { _ in
            XCTFail("Paste-only must not schedule Enter")
        }) { delivery, reason in
            XCTAssertEqual(delivery, "pasted"); XCTAssertNil(reason)
        }
        XCTAssertEqual(keys, [9])
    }

    func testEnterFollowsPasteWithNoModifiers() {
        var keys: [CGKeyCode] = []
        var pending: (() -> Void)?
        var completed = false
        Paste.send(shouldPressEnter: { true }, check: { nil }, isCurrent: { true }, post: { key, flags in
            keys.append(key)
            XCTAssertEqual(flags, key == 9 ? .maskCommand : [])
            return true
        }, afterPaste: { pending = $0 }) { delivery, reason in
            XCTAssertEqual(delivery, "submitted"); XCTAssertNil(reason); completed = true
        }
        XCTAssertEqual(keys, [9]); XCTAssertFalse(completed)
        pending?()
        XCTAssertEqual(keys, [9, 36]); XCTAssertTrue(completed)
    }

    func testNavigatingToExcludedSiteAfterPasteSuppressesReturn() {
        var allowed = true, keys: [CGKeyCode] = []
        Paste.send(shouldPressEnter: { allowed }, check: { nil }, isCurrent: { true }, post: { key, _ in
            keys.append(key); return true
        }, afterPaste: { callback in allowed = false; callback() }) { delivery, reason in
            XCTAssertEqual(delivery, "pasted"); XCTAssertNil(reason)
        }
        XCTAssertEqual(keys, [9])
    }

    func testBlockedOrFailedPasteNeverSendsEnter() {
        for blocked in [true, false] {
            var keys: [CGKeyCode] = []
            Paste.send(shouldPressEnter: { true }, check: { blocked ? .focusChanged : nil }, isCurrent: { true }, post: { key, _ in
                keys.append(key); return false
            }, afterPaste: { _ in XCTFail("Failed paste must not schedule Enter") }) { delivery, reason in
                XCTAssertEqual(delivery, "clipboard")
                XCTAssertEqual(reason, blocked ? .focusChanged : .eventUnavailable)
            }
            XCTAssertEqual(keys, blocked ? [] : [9])
        }
    }

    func testFocusChangeOrNewRecordingDuringDelaySkipsEnter() {
        for changedFocus in [true, false] {
            var keys: [CGKeyCode] = []
            var pending: (() -> Void)?
            var reason: PasteBlockReason?
            var current = true
            Paste.send(shouldPressEnter: { true }, check: { reason }, isCurrent: { current }, post: { key, _ in
                keys.append(key); return true
            }, afterPaste: { pending = $0 }) { delivery, _ in XCTAssertEqual(delivery, "pasted") }
            if changedFocus { reason = .focusChanged } else { current = false }
            pending?()
            XCTAssertEqual(keys, [9])
        }
    }

    func testPermissionFailureExplainsTextIsPreserved() {
        let reason = PasteBlockReason.check(trusted: false, targetAlive: true, targetIsSelf: false, targetIsFrontmost: true)
        XCTAssertEqual(reason, .accessibility)
        XCTAssertEqual(reason!.message(), "Text copied. Allow automatic paste.")
    }

    func testNeverPastesIntoChangedOrMissingTarget() {
        XCTAssertEqual(PasteBlockReason.check(trusted: true, targetAlive: false, targetIsSelf: false, targetIsFrontmost: false), .targetClosed)
        XCTAssertEqual(PasteBlockReason.check(trusted: true, targetAlive: true, targetIsSelf: false, targetIsFrontmost: false), .focusChanged)
        XCTAssertEqual(PasteBlockReason.check(trusted: true, targetAlive: true, targetIsSelf: true, targetIsFrontmost: true), .ownWindow)
        XCTAssertNil(PasteBlockReason.check(trusted: true, targetAlive: true, targetIsSelf: false, targetIsFrontmost: true))
    }
}
