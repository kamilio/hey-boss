import AppKit
import XCTest
import TalkCore
@testable import HeyTalk

final class PasteTests: XCTestCase {
    private var board: NSPasteboard!
    override func setUp() { board = NSPasteboard.withUniqueName() }
    override func tearDown() { board.releaseGlobally(); board = nil }

    func testRecoveryNoticeNamesOriginalAppAndCorrectAction() {
        XCTAssertEqual(PasteBlockReason.focusChanged.message(targetName: "Slack"), "Return to Slack. Press Hyper+V.")
        XCTAssertEqual(PasteBlockReason.targetClosed.message(targetName: "Notes"), "Reopen Notes. Press Hyper+V.")
        XCTAssertEqual(PasteBlockReason.focusChanged.message(targetName: "Slack", pasted: true), "Pasted in Slack. Focus changed before submit.")
        XCTAssertEqual(PasteBlockReason.focusChanged.message(targetName: nil), "Return to the original app. Press Hyper+V.")
    }

    func testPasteOnlyRestoresAllItemsAndFormatsAfterConsumption() {
        let first = NSPasteboardItem(), second = NSPasteboardItem()
        let rich = Data(#"{\rtf1\ansi Rich text}"#.utf8)
        first.setString("Previously copied", forType: .string)
        first.setData(rich, forType: .rtf)
        second.setString("file:///example.txt", forType: .fileURL)
        board.writeObjects([first, second])
        let originalTypes = Set(board.pasteboardItems![0].types)
        let generalCount = NSPasteboard.general.changeCount
        let text = String(repeating: "Unicode 🌍 and newlines\n", count: 100)
        var pending: (() -> Void)?
        var completed = false
        Paste.send(text, shouldPressEnter: { false }, check: { nil }, isCurrent: { true }, board: board, post: { key, flags in
            XCTAssertEqual(key, 9); XCTAssertEqual(flags, .maskCommand)
            XCTAssertEqual(self.board.string(forType: .string), text)
            XCTAssertTrue(self.board.types!.contains(NSPasteboard.PasteboardType("org.nspasteboard.TransientType")))
            return true
        }, afterPaste: { pending = $0 }) { delivery, reason in
            XCTAssertEqual(delivery, "pasted"); XCTAssertNil(reason); completed = true
        }
        XCTAssertFalse(completed)
        XCTAssertEqual(board.string(forType: .string), text)
        pending?()
        XCTAssertTrue(completed)
        XCTAssertEqual(board.pasteboardItems?.count, 2)
        XCTAssertEqual(board.pasteboardItems?[0].string(forType: .string), "Previously copied")
        XCTAssertEqual(board.pasteboardItems?[0].data(forType: .rtf), rich)
        XCTAssertEqual(board.pasteboardItems?[1].string(forType: .fileURL), "file:///example.txt")
        XCTAssertEqual(Set(board.pasteboardItems![0].types), originalTypes)
        XCTAssertEqual(NSPasteboard.general.changeCount, generalCount)
    }

    func testEnterFollowsPasteAndClipboardRestoration() {
        board.setString("original", forType: .string)
        var keys: [CGKeyCode] = []
        var pending: (() -> Void)?
        var completed = false
        Paste.send("Dictated text", shouldPressEnter: { true }, check: { nil }, isCurrent: { true }, board: board, post: { key, flags in
            keys.append(key)
            XCTAssertEqual(flags, key == 9 ? .maskCommand : [])
            if key == 36 { XCTAssertEqual(self.board.string(forType: .string), "original") }
            return true
        }, afterPaste: { pending = $0 }) { delivery, reason in
            XCTAssertEqual(delivery, "submitted"); XCTAssertNil(reason); completed = true
        }
        XCTAssertEqual(keys, [9]); XCTAssertFalse(completed)
        pending?()
        XCTAssertEqual(keys, [9, 36]); XCTAssertTrue(completed)
    }

    func testNewUserCopyIsNeverOverwritten() {
        board.setString("original", forType: .string)
        Paste.send("Dictated text", shouldPressEnter: { false }, check: { nil }, isCurrent: { true }, board: board,
                   post: { _, _ in true }, afterPaste: { callback in
            self.board.clearContents()
            self.board.setString("new user copy", forType: .string)
            callback()
        }) { delivery, reason in XCTAssertEqual(delivery, "pasted"); XCTAssertNil(reason) }
        XCTAssertEqual(board.string(forType: .string), "new user copy")
    }

    func testBlockedOrFailedPastePreservesClipboardAndNeverSendsEnter() {
        for blocked in [true, false] {
            board.clearContents(); board.setString("original", forType: .string)
            var keys: [CGKeyCode] = []
            let before = board.changeCount
            Paste.send("Dictated text", shouldPressEnter: { true }, check: { blocked ? .focusChanged : nil }, isCurrent: { true }, board: board, post: { key, _ in
                keys.append(key); return false
            }, afterPaste: { _ in XCTFail("Failed paste must not schedule Enter") }) { delivery, reason in
                XCTAssertEqual(delivery, "history")
                XCTAssertEqual(reason, blocked ? .focusChanged : .eventUnavailable)
            }
            XCTAssertEqual(keys, blocked ? [] : [9])
            XCTAssertEqual(board.string(forType: .string), "original")
            if blocked { XCTAssertEqual(board.changeCount, before) }
        }
    }

    func testFocusChangeCancellationOrSubmitDisabledStillRestoresClipboard() {
        for change in 0..<3 {
            board.clearContents(); board.setString("original", forType: .string)
            var keys: [CGKeyCode] = []
            var pending: (() -> Void)?
            var reason: PasteBlockReason?
            var current = true, submit = true
            Paste.send("Dictated text", shouldPressEnter: { submit }, check: { reason }, isCurrent: { current }, board: board, post: { key, _ in
                keys.append(key); return true
            }, afterPaste: { pending = $0 }) { delivery, _ in XCTAssertEqual(delivery, "pasted") }
            if change == 0 { reason = .focusChanged }
            else if change == 1 { current = false }
            else { submit = false }
            pending?()
            XCTAssertEqual(keys, [9])
            XCTAssertEqual(board.string(forType: .string), "original")
        }
    }

    func testRepeatedShortcutCannotCaptureTemporaryClipboardAsOriginal() {
        board.setString("original", forType: .string)
        var pending: (() -> Void)?
        Paste.send("first", shouldPressEnter: { false }, check: { nil }, isCurrent: { true }, board: board,
                   post: { _, _ in true }, afterPaste: { pending = $0 }) { _, _ in }
        Paste.send("second", shouldPressEnter: { false }, check: { nil }, isCurrent: { true }, board: board,
                   post: { _, _ in XCTFail("Overlapping paste"); return true }, afterPaste: { _ in XCTFail("Overlapping paste") }) { delivery, reason in
            XCTAssertEqual(delivery, "history"); XCTAssertEqual(reason, .eventUnavailable)
        }
        XCTAssertEqual(board.string(forType: .string), "first")
        pending?()
        XCTAssertEqual(board.string(forType: .string), "original")
    }

    func testEmptyClipboardAndFailedSubmitAreRestored() {
        Paste.send("Dictated text", shouldPressEnter: { true }, check: { nil }, isCurrent: { true }, board: board,
                   post: { key, _ in key == 9 }, afterPaste: { $0() }) { delivery, reason in
            XCTAssertEqual(delivery, "pasted"); XCTAssertEqual(reason, .eventUnavailable)
        }
        XCTAssertTrue(board.pasteboardItems?.isEmpty ?? true)
    }

    func testPermissionFailureExplainsTextIsPreserved() {
        let reason = PasteBlockReason.check(trusted: false, targetAlive: true, targetIsSelf: false, targetIsFrontmost: true)
        XCTAssertEqual(reason, .accessibility)
        XCTAssertEqual(reason!.message(), "Allow Accessibility, then press Hyper+V.")
    }

    func testNeverPastesIntoChangedOrMissingTarget() {
        XCTAssertEqual(PasteBlockReason.check(trusted: true, targetAlive: false, targetIsSelf: false, targetIsFrontmost: false), .targetClosed)
        XCTAssertEqual(PasteBlockReason.check(trusted: true, targetAlive: true, targetIsSelf: false, targetIsFrontmost: false), .focusChanged)
        XCTAssertEqual(PasteBlockReason.check(trusted: true, targetAlive: true, targetIsSelf: true, targetIsFrontmost: true), .ownWindow)
        XCTAssertNil(PasteBlockReason.check(trusted: true, targetAlive: true, targetIsSelf: false, targetIsFrontmost: true))
    }
}
