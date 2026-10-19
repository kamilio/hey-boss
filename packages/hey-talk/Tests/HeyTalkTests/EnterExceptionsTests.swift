import XCTest
@testable import HeyTalk

final class EnterExceptionsTests: XCTestCase {
    func testExceptionsPersistAndOnlySuppressEnterForTheTargetApp() throws {
        let suite = "hey-talk-test-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let settings = EnterExceptions(defaults: defaults)
        let slack = "com.tinyspeck.slackmacgap"
        XCTAssertEqual(try settings.command([]), [])
        XCTAssertTrue(settings.shouldPressEnter(requested: true, bundleID: slack))
        XCTAssertEqual(try settings.command(["add", slack]), [slack])
        XCTAssertEqual(try settings.command(["add", slack]), [slack])
        let reloaded = EnterExceptions(defaults: UserDefaults(suiteName: suite)!)
        XCTAssertFalse(reloaded.shouldPressEnter(requested: true, bundleID: slack))
        XCTAssertFalse(reloaded.shouldPressEnter(requested: false, bundleID: slack))
        XCTAssertTrue(reloaded.shouldPressEnter(requested: true, bundleID: "com.apple.TextEdit"))
        XCTAssertTrue(reloaded.shouldPressEnter(requested: true, bundleID: nil))
        XCTAssertFalse(reloaded.shouldPressEnter(requested: false, bundleID: "com.apple.TextEdit"))
        XCTAssertEqual(try settings.command(["remove", slack]), [])
        XCTAssertTrue(settings.shouldPressEnter(requested: true, bundleID: slack))
    }

    func testInvalidCommandsDoNotChangeExceptions() throws {
        let suite = "hey-talk-test-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let settings = EnterExceptions(defaults: defaults)
        _ = try settings.command(["add", "com.example.app"])
        for args in [["add"], ["reset", "com.example.app"], ["add", ""], ["add", "Slack"], ["remove", "com.example.app", "extra"]] {
            XCTAssertThrowsError(try settings.command(args))
            XCTAssertEqual(try settings.command([]), ["com.example.app"])
        }
    }
}
