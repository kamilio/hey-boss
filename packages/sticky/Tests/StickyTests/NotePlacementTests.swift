import XCTest
import AppKit
@testable import Sticky

final class NotePlacementTests: XCTestCase {
    private let laptop = NoteScreen(id: "laptop", frame: NSRect(x: 0, y: 0, width: 1512, height: 982),
                                    visibleFrame: NSRect(x: 0, y: 38, width: 1512, height: 906), builtIn: true)
    private let external = NoteScreen(id: "external", frame: NSRect(x: 1512, y: 0, width: 2560, height: 1440),
                                      visibleFrame: NSRect(x: 1512, y: 38, width: 2560, height: 1377), builtIn: false)
    private var defaults: UserDefaults!
    private var suite: String!

    override func setUp() {
        suite = "StickyPlacementTests." + UUID().uuidString
        defaults = UserDefaults(suiteName: suite)
    }
    override func tearDown() { defaults.removePersistentDomain(forName: suite) }

    func testDisconnectMoveOnLaptopReconnectAndRestart() throws {
        let placement = NotePlacement(id: "focus", position: 0, defaults: defaults)
        let deskFrame = NSRect(x: 3400, y: 800, width: 500, height: 500)
        placement.save(deskFrame, on: [laptop, external])
        XCTAssertEqual(placement.frame(on: [laptop, external]), deskFrame)
        XCTAssertTrue(laptop.visibleFrame.contains(try XCTUnwrap(placement.frame(on: [laptop]))))
        let laptopFrame = NSRect(x: 800, y: 300, width: 400, height: 420)
        placement.save(laptopFrame, on: [laptop])
        XCTAssertEqual(placement.frame(on: [laptop]), laptopFrame)
        XCTAssertEqual(placement.frame(on: [laptop, external]), deskFrame)
        let reopened = NotePlacement(id: "focus", position: 0, defaults: defaults)
        XCTAssertEqual(reopened.frame(on: [laptop, external]), deskFrame)
        XCTAssertEqual(reopened.frame(on: [laptop]), laptopFrame)
    }

    func testMovingToLaptopWhileBothConnectedChangesPreferredDisplay() {
        let placement = NotePlacement(id: "focus", position: 0, defaults: defaults)
        placement.save(NSRect(x: 3400, y: 800, width: 400, height: 420), on: [laptop, external])
        let frame = NSRect(x: 800, y: 300, width: 400, height: 420)
        placement.save(frame, on: [laptop, external])
        XCTAssertEqual(placement.frame(on: [laptop, external]), frame)
    }

    func testDisplayRearrangementAndSmallerResolutionDoNotErasePosition() throws {
        let placement = NotePlacement(id: "focus", position: 0, defaults: defaults)
        placement.save(NSRect(x: 3400, y: 800, width: 500, height: 500), on: [laptop, external])
        let rearranged = NoteScreen(id: external.id, frame: external.frame.offsetBy(dx: -4072, dy: 100),
                                   visibleFrame: external.visibleFrame.offsetBy(dx: -4072, dy: 100), builtIn: false)
        XCTAssertEqual(placement.frame(on: [laptop, rearranged]), NSRect(x: -672, y: 900, width: 500, height: 500))
        let smaller = NoteScreen(id: external.id, frame: NSRect(x: 1512, y: 0, width: 1280, height: 720),
                                visibleFrame: NSRect(x: 1512, y: 38, width: 1280, height: 657), builtIn: false)
        XCTAssertTrue(smaller.visibleFrame.contains(try XCTUnwrap(placement.frame(on: [laptop, smaller]))))
        XCTAssertEqual(placement.frame(on: [laptop, external]), NSRect(x: 3400, y: 800, width: 500, height: 500))
    }

    func testLegacyFrameMigrationAndUnseenScreenDefaults() throws {
        let placement = NotePlacement(id: "focus", position: 0, defaults: defaults)
        let frame = NSRect(x: 3400, y: 800, width: 400, height: 420)
        defaults.set(NSStringFromRect(frame), forKey: "frame.focus")
        XCTAssertEqual(placement.frame(on: [laptop, external]), frame)
        XCTAssertNil(defaults.object(forKey: "frame.focus"))
        let newNote = NotePlacement(id: "new", position: 0, defaults: defaults)
        let initial = try XCTUnwrap(newNote.frame(on: [laptop, external]))
        XCTAssertEqual(initial.maxX, external.visibleFrame.maxX - 32)
        XCTAssertEqual(initial.maxY, external.visibleFrame.maxY - 40)
        XCTAssertTrue(laptop.visibleFrame.contains(try XCTUnwrap(newNote.frame(on: [laptop]))))
        XCTAssertNil(newNote.frame(on: []))
    }
}
