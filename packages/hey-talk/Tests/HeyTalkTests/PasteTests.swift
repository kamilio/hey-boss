import XCTest
@testable import HeyTalk

final class PasteTests: XCTestCase {
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
