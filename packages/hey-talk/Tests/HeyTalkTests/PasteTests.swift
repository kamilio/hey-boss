import XCTest
@testable import HeyTalk

final class PasteTests: XCTestCase {
    func testPasteOnlyNeverSchedulesEnter() {
        var keys: [CGKeyCode] = []
        Paste.send(pressEnter: false, check: { nil }, isCurrent: { true }, post: { key, _ in keys.append(key); return true }, afterPaste: { _ in
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
        Paste.send(pressEnter: true, check: { nil }, isCurrent: { true }, post: { key, flags in
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

    func testBlockedOrFailedPasteNeverSendsEnter() {
        for blocked in [true, false] {
            var keys: [CGKeyCode] = []
            Paste.send(pressEnter: true, check: { blocked ? .focusChanged : nil }, isCurrent: { true }, post: { key, _ in
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
            Paste.send(pressEnter: true, check: { reason }, isCurrent: { current }, post: { key, _ in
                keys.append(key); return true
            }, afterPaste: { pending = $0 }) { delivery, _ in XCTAssertEqual(delivery, "pasted") }
            if changedFocus { reason = .focusChanged } else { current = false }
            pending?()
            XCTAssertEqual(keys, [9])
        }
    }

    func testPermissionFailureExplainsStaleSettingsEntry() {
        let reason = PasteBlockReason.check(trusted: false, targetAlive: true, targetIsSelf: false, targetIsFrontmost: true)
        XCTAssertEqual(reason, .accessibility)
        XCTAssertTrue(reason!.message.contains("Accessibility"))
    }

    func testNeverPastesIntoChangedOrMissingTarget() {
        XCTAssertEqual(PasteBlockReason.check(trusted: true, targetAlive: false, targetIsSelf: false, targetIsFrontmost: false), .targetClosed)
        XCTAssertEqual(PasteBlockReason.check(trusted: true, targetAlive: true, targetIsSelf: false, targetIsFrontmost: false), .focusChanged)
        XCTAssertEqual(PasteBlockReason.check(trusted: true, targetAlive: true, targetIsSelf: true, targetIsFrontmost: true), .ownWindow)
        XCTAssertNil(PasteBlockReason.check(trusted: true, targetAlive: true, targetIsSelf: false, targetIsFrontmost: true))
    }
}
