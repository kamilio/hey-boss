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

    func testSiteDefaultsBoundariesPersistenceAndAppPrecedence() throws {
        let suite = "hey-talk-test-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let settings = EnterExceptions(defaults: defaults)
        XCTAssertEqual(try settings.command(["sites"]), ["latticehq.com"])
        for host in ["LATTICEHQ.COM.", "team.latticehq.com", "a.b.latticehq.com"] {
            XCTAssertFalse(settings.shouldPressEnter(requested: true, bundleID: "com.google.Chrome", host: host))
        }
        for host in ["lattice.com", "team.lattice.com", "notlatticehq.com", "latticehq.com.example.org"] {
            XCTAssertTrue(settings.shouldPressEnter(requested: true, bundleID: "com.apple.Safari", host: host))
        }
        _ = try settings.command(["sites", "remove", "LatticeHQ.com"])
        _ = try settings.command(["sites", "add", "Example.org"])
        XCTAssertEqual(try EnterExceptions(defaults: defaults).command(["sites"]), ["example.org"])
        for bad in ["https://example.org", "user@example.org", "example.org/path", "a..b", "-bad.org"] {
            XCTAssertThrowsError(try settings.command(["sites", "add", bad]))
        }
        _ = try settings.command(["add", "com.google.Chrome"])
        XCTAssertFalse(settings.shouldPressEnter(requested: true, bundleID: "com.google.Chrome", host: "allowed.net"))
        XCTAssertFalse(settings.shouldPressEnter(requested: false, bundleID: "com.apple.Safari", host: "allowed.net"))
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
