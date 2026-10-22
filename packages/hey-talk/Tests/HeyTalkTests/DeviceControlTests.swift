import XCTest
import TalkCore
@testable import HeyTalk

final class DeviceControlTests: XCTestCase {
    func testSubmitFlagIsExplicitAndRejectsConflictingOrUnrelatedOptions() throws {
        XCTAssertNil(try CLI.submitOption(["toggle"]))
        XCTAssertEqual(try CLI.submitOption(["toggle", "--submit", "--json"]), true)
        for verb in ["start", "stop", "record"] {
            XCTAssertEqual(try CLI.submitOption([verb, "--submit"]), true)
        }
        for args in [["toggle", "--submit", "--no-paste"], ["status", "--submit"], ["auto", "--submit"], ["toggle", "--submit", "--submit"]] {
            XCTAssertThrowsError(try CLI.submitOption(args))
        }
    }
    func testSubmitToggleMatchesStoppingShortcutAndNeverRestartsDuringDecode() {
        XCTAssertEqual(DictationToggle.resolve(phase: .idle, currentSubmit: false, requestedSubmit: true), .start(submit: true))
        XCTAssertEqual(DictationToggle.resolve(phase: .recording, currentSubmit: false, requestedSubmit: true), .stop(submit: true))
        XCTAssertEqual(DictationToggle.resolve(phase: .recording, currentSubmit: true, requestedSubmit: nil), .stop(submit: true))
        XCTAssertEqual(DictationToggle.resolve(phase: .recording, currentSubmit: true, requestedSubmit: false), .stop(submit: false))
        XCTAssertEqual(DictationToggle.resolve(phase: .idle, currentSubmit: true, requestedSubmit: nil), .start(submit: false))
        XCTAssertEqual(DictationToggle.resolve(phase: .processing, currentSubmit: true, requestedSubmit: true), .busy)
    }
    func testAutoToggleRunsInsideSettingsAndPreservesAllowlist() throws {
        let name = "hey-talk-test-\(UUID())"
        let defaults = UserDefaults(suiteName: name)!
        defer { defaults.removePersistentDomain(forName: name) }
        let settings = AutoDictationSettings(defaults: defaults)
        try settings.command(["toggle"])
        XCTAssertTrue(settings.enabled)
        XCTAssertEqual(settings.bundleIDs, ["com.t3tools.t3code"])
        try settings.command(["toggle"])
        XCTAssertFalse(settings.enabled)
        XCTAssertThrowsError(try settings.command(["toggle", "extra"]))
    }
}
