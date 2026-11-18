import AppKit
import XCTest
@testable import HeyTalk

final class PasteTests: XCTestCase {
    func testCorrectionFailureRecoversExactTranscriptWithoutPasting() {
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        board.setString("Previous clipboard", forType: .string)
        let original = "raw dictation…\nsecond line"

        XCTAssertEqual(Paste.recoverTranscript(original, to: nil).delivery, "stdout")
        XCTAssertEqual(board.string(forType: .string), "Previous clipboard")

        let result = Paste.recoverTranscript(original, to: board)
        XCTAssertEqual(result.delivery, "clipboard")
        XCTAssertNil(result.reason)
        XCTAssertEqual(board.string(forType: .string), original)
    }
    private var board: NSPasteboard!
    override func setUp() { board = NSPasteboard.withUniqueName() }
    override func tearDown() { board.releaseGlobally(); board = nil }

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
        Paste.send("Dictated text", to: board, shouldPressEnter: { false }, check: { nil }, isCurrent: { true }, insert: { _ in true }, post: { key, _ in keys.append(key); return true }, afterPaste: { _ in
            XCTFail("Paste-only must not schedule Enter")
        }) { delivery, reason in
            XCTAssertEqual(delivery, "pasted"); XCTAssertNil(reason)
        }
        XCTAssertEqual(keys, [])
    }

    func testEnterFollowsPasteWithNoModifiers() {
        var keys: [CGKeyCode] = []
        var pending: (() -> Void)?
        var completed = false
        Paste.send("Dictated text", to: board, shouldPressEnter: { true }, check: { nil }, isCurrent: { true }, insert: { _ in true }, post: { key, flags in
            keys.append(key)
            XCTAssertEqual(flags, [])
            return true
        }, afterPaste: { pending = $0 }) { delivery, reason in
            XCTAssertEqual(delivery, "submitted"); XCTAssertNil(reason); completed = true
        }
        XCTAssertEqual(keys, []); XCTAssertFalse(completed)
        pending?()
        XCTAssertEqual(keys, [36]); XCTAssertTrue(completed)
    }

    func testNavigatingToExcludedSiteAfterPasteSuppressesReturn() {
        var allowed = true, keys: [CGKeyCode] = []
        Paste.send("Dictated text", to: board, shouldPressEnter: { allowed }, check: { nil }, isCurrent: { true }, insert: { _ in true }, post: { key, _ in
            keys.append(key); return true
        }, afterPaste: { callback in allowed = false; callback() }) { delivery, reason in
            XCTAssertEqual(delivery, "pasted"); XCTAssertNil(reason)
        }
        XCTAssertEqual(keys, [])
    }

    func testBlockedOrFailedPasteNeverSendsEnter() {
        for blocked in [true, false] {
            var keys: [CGKeyCode] = []
            Paste.send("Dictated text", to: board, shouldPressEnter: { true }, check: { blocked ? .focusChanged : nil }, isCurrent: { true }, insert: { _ in
                XCTAssertFalse(blocked); return false
            }, post: { key, _ in
                keys.append(key); return false
            }, afterPaste: { _ in XCTFail("Failed paste must not schedule Enter") }) { delivery, reason in
                XCTAssertEqual(delivery, "clipboard")
                XCTAssertEqual(reason, blocked ? .focusChanged : .eventUnavailable)
            }
            XCTAssertEqual(keys, [])
            XCTAssertEqual(board.string(forType: .string), "Dictated text")
        }
    }

    func testFocusChangeOrNewRecordingDuringDelaySkipsEnter() {
        for changedFocus in [true, false] {
            var keys: [CGKeyCode] = []
            var pending: (() -> Void)?
            var reason: PasteBlockReason?
            var current = true
            Paste.send("Dictated text", to: board, shouldPressEnter: { true }, check: { reason }, isCurrent: { current }, insert: { _ in true }, post: { key, _ in
                keys.append(key); return true
            }, afterPaste: { pending = $0 }) { delivery, _ in XCTAssertEqual(delivery, "pasted") }
            if changedFocus { reason = .focusChanged } else { current = false }
            pending?()
            XCTAssertEqual(keys, [])
        }
    }

    func testPermissionFailureExplainsTextIsPreserved() {
        let reason = PasteBlockReason.check(trusted: false, targetAlive: true, targetIsSelf: false, targetIsFrontmost: true)
        XCTAssertEqual(reason, .accessibility)
        XCTAssertEqual(reason!.message(), "Text copied. Allow automatic paste.")
    }

    func testSuccessfulInsertionNeverTouchesClipboard() {
        let first = NSPasteboardItem(), second = NSPasteboardItem()
        let rich = Data([0, 1, 2, 255])
        first.setString("Previously copied", forType: .string)
        first.setData(rich, forType: .rtf)
        second.setString("file:///example.txt", forType: .fileURL)
        board.writeObjects([first, second])
        let before = board.changeCount
        let text = String(repeating: "Unicode 🌍 and newlines\n", count: 100)
        Paste.send(text, to: board, shouldPressEnter: { false }, check: { nil }, isCurrent: { true }, insert: { inserted in
            XCTAssertEqual(inserted, text)
            XCTAssertEqual(self.board.changeCount, before)
            return true
        }, post: { _, _ in XCTFail("No keyboard events needed"); return false },
                   afterPaste: { _ in XCTFail("No submit needed") }) { delivery, reason in
            XCTAssertEqual(delivery, "pasted"); XCTAssertNil(reason)
        }
        XCTAssertEqual(board.changeCount, before)
        XCTAssertEqual(board.pasteboardItems?.count, 2)
        XCTAssertEqual(board.pasteboardItems?[0].string(forType: .string), "Previously copied")
        XCTAssertEqual(board.pasteboardItems?[0].data(forType: .rtf), rich)
        XCTAssertEqual(board.pasteboardItems?[1].string(forType: .fileURL), "file:///example.txt")
    }

    func testFailedSubmitDoesNotCopyAlreadyInsertedText() {
        let before = board.changeCount
        Paste.send("Dictated text", to: board, shouldPressEnter: { true }, check: { nil }, isCurrent: { true },
                   insert: { _ in true }, post: { key, flags in
            XCTAssertEqual(key, 36); XCTAssertEqual(flags, []); return false
        }, afterPaste: { $0() }) { delivery, reason in
            XCTAssertEqual(delivery, "pasted"); XCTAssertEqual(reason, .eventUnavailable)
        }
        XCTAssertEqual(board.changeCount, before)
    }

    @MainActor
    func testLiveInsertionReplacesSelectionWithoutClipboard() async throws {
        guard let bundleID = ProcessInfo.processInfo.environment["HEY_TALK_INSERT_TEST"] else { throw XCTSkip("Requires a focused scratch field") }
        let app = try XCTUnwrap(NSRunningApplication.runningApplications(withBundleIdentifier: bundleID).first)
        let field = try XCTUnwrap(FocusedInput.read(app: app, forAuto: false))
        let marker = "Before 👋 selected after"
        guard SpeechAX.string(field.element, "AXValue").trimmingCharacters(in: .newlines) == marker else {
            return XCTFail("Focus a field containing only the fixture")
        }
        let before = board.changeCount
        for range in [CFRange(location: 10, length: 8), CFRange(location: 10, length: 0)] {
            var range = range
            let selected = try XCTUnwrap(AXValueCreate(.cfRange, &range))
            XCTAssertEqual(AXUIElementSetAttributeValue(field.element, kAXSelectedTextRangeAttribute as CFString, selected), .success)
            let text = "inserted 🌍\nsecond line"
            let original = SpeechAX.string(field.element, "AXValue") as NSString
            let expected = original.replacingCharacters(in: NSRange(location: range.location, length: range.length), with: text)
            let done = expectation(description: "inserted")
            Paste.send(text, to: board, shouldPressEnter: { false }, check: { nil }, isCurrent: { true },
                       insert: { Paste.insert($0, into: field.element) },
                       post: { _, _ in XCTFail("No keyboard events needed"); return false },
                       afterPaste: { _ in XCTFail("No submit needed") }) { delivery, reason in
                XCTAssertEqual(delivery, "pasted"); XCTAssertNil(reason); done.fulfill()
            }
            await fulfillment(of: [done], timeout: 3)
            for _ in 0..<20 {
                if SpeechAX.string(field.element, "AXValue") == expected { break }
                try await Task.sleep(nanoseconds: 25_000_000)
            }
            XCTAssertEqual(SpeechAX.string(field.element, "AXValue"), expected)
            XCTAssertEqual(board.changeCount, before)
        }
    }

    func testNeverPastesIntoChangedOrMissingTarget() {
        XCTAssertEqual(PasteBlockReason.check(trusted: true, targetAlive: false, targetIsSelf: false, targetIsFrontmost: false), .targetClosed)
        XCTAssertEqual(PasteBlockReason.check(trusted: true, targetAlive: true, targetIsSelf: false, targetIsFrontmost: false), .focusChanged)
        XCTAssertEqual(PasteBlockReason.check(trusted: true, targetAlive: true, targetIsSelf: true, targetIsFrontmost: true), .ownWindow)
        XCTAssertNil(PasteBlockReason.check(trusted: true, targetAlive: true, targetIsSelf: false, targetIsFrontmost: true))
    }
}
