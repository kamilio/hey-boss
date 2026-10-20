import AppKit
import TalkCore
import XCTest
@testable import HeyTalk

final class AutoDictationTests: XCTestCase {
    func testSettingsDefaultToManualWithT3AllowedAndPersistChanges() throws {
        let suite = "hey-talk-test-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let settings = AutoDictationSettings(defaults: defaults)
        XCTAssertFalse(settings.enabled)
        XCTAssertEqual(settings.bundleIDs, ["com.t3tools.t3code"])
        try settings.command(["on"])
        try settings.command(["add", "com.example.Editor"])
        try settings.command(["add", "com.example.Editor"])
        let reloaded = AutoDictationSettings(defaults: UserDefaults(suiteName: suite)!)
        XCTAssertTrue(reloaded.enabled)
        XCTAssertEqual(reloaded.bundleIDs, ["com.example.Editor", "com.t3tools.t3code"])
        for args in [["add"], ["on", "extra"], ["add", "bad id"], ["reset"]] {
            XCTAssertThrowsError(try settings.command(args))
        }
        try settings.command(["remove", "com.t3tools.t3code"])
        try settings.command(["off"])
        XCTAssertFalse(reloaded.enabled)
        XCTAssertEqual(reloaded.bundleIDs, ["com.example.Editor"])
    }

    func testFocusStartsOnceAndDoesNotRestartAfterCancelOrFailure() {
        var trigger = FocusTrigger<String>()
        XCTAssertTrue(trigger.update("composer", available: true))
        XCTAssertFalse(trigger.update("composer", available: false))
        XCTAssertFalse(trigger.update("composer", available: true))
        XCTAssertFalse(trigger.update(nil, available: true))
        XCTAssertTrue(trigger.update("composer", available: true))
        XCTAssertTrue(trigger.update("other-field", available: true))
    }

    func testRecordingSurvivesSwitchingFieldsAndUsesTheFieldWhereItFinishes() {
        var recording = AutoRecording(target: "first composer")
        recording.focusChanged(nil, at: 10)
        XCTAssertFalse(recording.shouldCancel(at: 11.9))
        recording.focusChanged("second composer", at: 11.9)
        XCTAssertFalse(recording.shouldCancel(at: 20))
        XCTAssertEqual(recording.target, "second composer")
        recording.finish(in: "third composer")
        XCTAssertEqual(recording.target, "third composer")
        XCTAssertTrue(recording.deliveryValid)
        recording.focusChanged("fourth composer", at: 21)
        XCTAssertEqual(recording.target, "third composer")
        XCTAssertFalse(recording.deliveryValid)
        recording.focusChanged("third composer", at: 22)
        XCTAssertFalse(recording.deliveryValid, "Returning must not undo a focus change during decoding")
    }

    func testFocusGraceExpiresOnceAndResetsAfterReturningToAnInput() {
        var recording = AutoRecording(target: "composer")
        recording.focusChanged(nil, at: 10)
        recording.focusChanged(nil, at: 11)
        XCTAssertTrue(recording.shouldCancel(at: 12))
        recording.focusChanged("composer", at: 12)
        XCTAssertFalse(recording.shouldCancel(at: 13))
        recording.focusChanged(nil, at: 14)
        XCTAssertFalse(recording.shouldCancel(at: 15.9))
        XCTAssertTrue(recording.shouldCancel(at: 16))
    }

    func testSuccessfulSubmitCanRearmTheSameFieldWithoutAFocusChange() {
        var trigger = FocusTrigger<String>()
        XCTAssertTrue(trigger.update("composer", available: true))
        XCTAssertFalse(trigger.update("composer", available: true))
        trigger.reset()
        XCTAssertTrue(trigger.update("composer", available: true))
    }

    func testFocusDuringManualRecordingOrProcessingDoesNotQueueAnotherRecording() {
        var trigger = FocusTrigger<String>()
        XCTAssertFalse(trigger.update("composer", available: false))
        XCTAssertFalse(trigger.update("composer", available: true))
        trigger.reset()
        XCTAssertTrue(trigger.update("composer", available: true))
    }

    func testOnlyEditableNonSecureInputsQualifyAndT3SearchDoesNot() {
        func qualifies(_ role: String = "AXTextArea", subrole: String? = nil, enabled: Bool = true,
                       editable: Bool = true, label: String = "Message", app: String = "com.t3tools.t3code") -> Bool {
            FocusedInput.isEligible(bundleID: app, role: role, subrole: subrole, enabled: enabled, editable: editable, label: label)
        }
        XCTAssertTrue(qualifies())
        XCTAssertFalse(qualifies(label: "Search threads"))
        XCTAssertFalse(qualifies("AXButton"))
        XCTAssertFalse(qualifies(enabled: false))
        XCTAssertFalse(qualifies(editable: false))
        XCTAssertFalse(qualifies("AXTextField", subrole: "AXSecureTextField", app: "com.example.Editor"))
        XCTAssertTrue(qualifies("AXTextField", label: "Title", app: "com.example.Editor"))
    }

    func testEnterIsConsumedOnceIncludingRepeatsAndRelease() {
        for key in [36, 76] {
            var state = AutoSubmitKeyState()
            XCTAssertEqual(state.handle(key: key, down: true, modified: false, repeated: false, capture: true), .submit)
            XCTAssertEqual(state.handle(key: key, down: true, modified: false, repeated: true, capture: true), .consume)
            // Still swallow the matching release if dictation ended or focus moved.
            XCTAssertEqual(state.handle(key: key, down: false, modified: false, repeated: false, capture: false), .consume)
            XCTAssertEqual(state.handle(key: key, down: true, modified: false, repeated: false, capture: false), .pass)
        }
    }

    func testModifiedEnterOtherKeysAndPreexistingKeyRepeatPassThrough() {
        var state = AutoSubmitKeyState()
        XCTAssertEqual(state.handle(key: 36, down: true, modified: true, repeated: false, capture: true), .pass)
        XCTAssertEqual(state.handle(key: 36, down: true, modified: false, repeated: true, capture: true), .pass)
        XCTAssertEqual(state.handle(key: 9, down: true, modified: false, repeated: false, capture: true), .pass)
        XCTAssertEqual(state.handle(key: 36, down: false, modified: false, repeated: false, capture: true), .pass)
    }

    func testEnterAfterNoSpeechPassesOnceWithRepeatsAndRelease() {
        for key in [36, 76] {
            var state = AutoSubmitKeyState()
            XCTAssertEqual(state.handle(key: key, down: true, modified: false, repeated: false, capture: true), .submit)
            state.passNextEnter = true
            // The original captured press must still swallow its repeat and release.
            XCTAssertEqual(state.handle(key: key, down: true, modified: false, repeated: true, capture: true), .consume)
            XCTAssertEqual(state.handle(key: key, down: false, modified: false, repeated: false, capture: true), .consume)
            XCTAssertTrue(state.passNextEnter)

            XCTAssertEqual(state.handle(key: key, down: true, modified: false, repeated: false, capture: true), .pass)
            XCTAssertFalse(state.passNextEnter)
            XCTAssertEqual(state.handle(key: key, down: true, modified: false, repeated: true, capture: true), .pass)
            XCTAssertEqual(state.handle(key: key, down: false, modified: false, repeated: false, capture: true), .pass)
            XCTAssertEqual(state.handle(key: key, down: true, modified: false, repeated: false, capture: true), .submit)
        }
    }

    func testOtherKeysAndInputsDoNotUseTheNoSpeechPassThrough() {
        var state = AutoSubmitKeyState()
        state.passNextEnter = true
        XCTAssertEqual(state.handle(key: 36, down: true, modified: true, repeated: false, capture: true), .pass)
        XCTAssertEqual(state.handle(key: 14, down: true, modified: false, repeated: false, capture: true), .pass)
        XCTAssertEqual(state.handle(key: 36, down: true, modified: false, repeated: false, capture: false), .pass)
        XCTAssertTrue(state.passNextEnter)
        XCTAssertEqual(state.handle(key: 36, down: true, modified: false, repeated: false, capture: true), .pass)
        XCTAssertFalse(state.passNextEnter)
    }

    func testEventHandlerPassesEnterAfterNoSpeechWithoutSubmittingAgain() throws {
        let key = AutoSubmitKey()
        var submissions = 0
        key.shouldCapture = { true }
        key.onSubmit = { submissions += 1 }
        let enter = try XCTUnwrap(CGEvent(keyboardEventSource: nil, virtualKey: 36, keyDown: true))
        enter.flags = []
        XCTAssertNil(key.handle(.keyDown, enter))
        XCTAssertNil(key.handle(.keyUp, enter))
        XCTAssertEqual(submissions, 1)
        key.passNextEnter = true

        let generated = try XCTUnwrap(CGEvent(keyboardEventSource: nil, virtualKey: 36, keyDown: true))
        generated.setIntegerValueField(.eventSourceUserData, value: Paste.eventTag)
        XCTAssertNotNil(key.handle(.keyDown, generated))
        XCTAssertTrue(key.passNextEnter, "Generated Return must not use the physical-key pass-through")
        XCTAssertNotNil(key.handle(.keyDown, enter))
        XCTAssertNotNil(key.handle(.keyUp, enter))
        XCTAssertFalse(key.passNextEnter)
        XCTAssertEqual(submissions, 1, "Passing Enter through must leave the new recording alone")
        XCTAssertNil(key.handle(.keyDown, enter))
        XCTAssertEqual(submissions, 2)
        XCTAssertNil(key.handle(.keyUp, enter))

        key.passNextEnter = true
        key.stop()
        XCTAssertFalse(key.passNextEnter, "Turning Auto off must clear the pending pass-through")
    }

    func testFocusCheckCanClearPassThroughBeforeHandlingTheKey() throws {
        let key = AutoSubmitKey()
        key.passNextEnter = true
        key.shouldCapture = { key.passNextEnter = false; return true }
        defer { key.shouldCapture = { false } }
        let enter = try XCTUnwrap(CGEvent(keyboardEventSource: nil, virtualKey: 36, keyDown: true))
        enter.flags = []
        XCTAssertNil(key.handle(.keyDown, enter))
        XCTAssertFalse(key.passNextEnter)
        XCTAssertNil(key.handle(.keyUp, enter))
    }

    @MainActor func testSilentRetryEnterReachesTextInputWhileRecordingContinues() throws {
        _ = NSApplication.shared
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 200, height: 100),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        let editor = NSTextView(frame: window.contentView!.bounds)
        window.contentView = editor
        window.makeFirstResponder(editor)
        defer { window.orderOut(nil) }
        var session = RecordingSession()
        let key = AutoSubmitKey()
        key.shouldCapture = { session.phase == .recording || key.passNextEnter }
        defer { key.shouldCapture = { false } }
        key.onSubmit = { try? session.processing(session.generation) }
        func pressEnter() throws {
            let down = try XCTUnwrap(CGEvent(keyboardEventSource: nil, virtualKey: 36, keyDown: true))
            down.flags = []
            down.keyboardSetUnicodeString(stringLength: 1, unicodeString: [13])
            if let forwarded = key.handle(.keyDown, down) {
                editor.keyDown(with: try XCTUnwrap(NSEvent(cgEvent: forwarded.takeUnretainedValue())))
            }
            let up = try XCTUnwrap(CGEvent(keyboardEventSource: nil, virtualKey: 36, keyDown: false))
            _ = key.handle(.keyUp, up)
        }
        let attempt = try session.begin()
        try pressEnter()
        XCTAssertEqual(session.phase, .processing)
        XCTAssertEqual(editor.string, "")

        XCTAssertTrue(session.finish(attempt, text: ""))
        key.passNextEnter = true
        _ = try session.begin()
        try pressEnter()
        XCTAssertEqual(editor.string, "\n")
        XCTAssertEqual(session.phase, .recording)
        try pressEnter()
        XCTAssertEqual(editor.string, "\n")
        XCTAssertEqual(session.phase, .processing)

        // The second Enter can also arrive during the microphone restart delay.
        XCTAssertTrue(session.finish(session.generation, text: ""))
        key.passNextEnter = true
        try pressEnter()
        XCTAssertEqual(editor.string, "\n\n")
        XCTAssertEqual(session.phase, .idle)
        _ = try session.begin()
        try pressEnter()
        XCTAssertEqual(editor.string, "\n\n")
        XCTAssertEqual(session.phase, .processing)
    }

    func testEventHandlerLetsOurSubmitThroughAndConsumesOnlyPhysicalPlainEnter() throws {
        let key = AutoSubmitKey()
        var submissions = 0, focusChecks = 0
        key.shouldCapture = { focusChecks += 1; return true }
        key.onSubmit = { submissions += 1 }
        let generated = try XCTUnwrap(CGEvent(keyboardEventSource: nil, virtualKey: 36, keyDown: true))
        generated.setIntegerValueField(.eventSourceUserData, value: Paste.eventTag)
        XCTAssertNotNil(key.handle(.keyDown, generated))
        XCTAssertEqual(focusChecks, 0)
        let enter = try XCTUnwrap(CGEvent(keyboardEventSource: nil, virtualKey: 36, keyDown: true))
        XCTAssertEqual(enter.getIntegerValueField(.eventSourceUserData), 0)
        enter.flags = .maskShift
        XCTAssertNotNil(key.handle(.keyDown, enter))
        XCTAssertEqual(focusChecks, 0)
        enter.flags = []
        XCTAssertNil(key.handle(.keyDown, enter))
        XCTAssertEqual(submissions, 1)
        enter.setIntegerValueField(.keyboardEventAutorepeat, value: 1)
        XCTAssertNil(key.handle(.keyDown, enter))
        XCTAssertNil(key.handle(.keyUp, enter))
        XCTAssertEqual(focusChecks, 1)
        XCTAssertEqual(submissions, 1)
        key.shouldCapture = { false }
        enter.setIntegerValueField(.keyboardEventAutorepeat, value: 0)
        XCTAssertNotNil(key.handle(.keyDown, enter))
    }
}
