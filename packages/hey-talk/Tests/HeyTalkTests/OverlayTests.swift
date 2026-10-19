import AppKit
import XCTest
@testable import HeyTalk

final class OverlayTests: XCTestCase {
    @MainActor func testWaveformIsVisibleAsSoonAsRecordingIsReady() throws {
        let app = NSApplication.shared
        let oldWindows = Set(app.windows.map(\.windowNumber))
        let overlay = Overlay()
        defer { overlay.show(false) }
        let panel = try XCTUnwrap(app.windows.first { !oldWindows.contains($0.windowNumber) && $0.title == "Hey Talk — Recording" })
        func findWave(_ view: NSView) -> NSView? {
            if view.identifier?.rawValue == "recording-waveform" { return view }
            return view.subviews.lazy.compactMap(findWave).first
        }
        let wave = try XCTUnwrap(panel.contentView.flatMap(findWave))
        overlay.preparing()
        XCTAssertTrue(wave.isHidden)
        overlay.update(.recording)
        XCTAssertFalse(wave.isHidden)
        XCTAssertEqual(wave.alphaValue, 1)
        overlay.update(.idle)
        XCTAssertTrue(wave.isHidden)
    }
}
