import AppKit

struct NoteScreen {
    let id: String
    let frame: NSRect
    let visibleFrame: NSRect
    let builtIn: Bool

    static var current: [NoteScreen] {
        NSScreen.screens.compactMap { screen in
            guard let number = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber,
                  let uuid = CGDisplayCreateUUIDFromDisplayID(number.uint32Value)?.takeRetainedValue() else { return nil }
            return NoteScreen(id: CFUUIDCreateString(nil, uuid) as String, frame: screen.frame,
                              visibleFrame: screen.visibleFrame, builtIn: CGDisplayIsBuiltin(number.uint32Value) != 0)
        }
    }
}

struct NotePlacement {
    let id: String
    let position: Int
    var defaults: UserDefaults = .standard

    func save(_ frame: NSRect, on screens: [NoteScreen]) {
        guard let screen = screen(containing: frame, in: screens) else { return }
        var frames = defaults.dictionary(forKey: "frames.\(id)") as? [String: String] ?? [:]
        frames[screen.id] = NSStringFromRect(frame.offsetBy(dx: -screen.frame.minX, dy: -screen.frame.minY))
        defaults.set(frames, forKey: "frames.\(id)")
        let preferred = defaults.string(forKey: "screen.\(id)")
        if preferred == nil || screens.contains(where: { $0.id == preferred }) {
            defaults.set(screen.id, forKey: "screen.\(id)")
        }
        defaults.removeObject(forKey: "frame.\(id)")
    }

    func frame(on screens: [NoteScreen]) -> NSRect? {
        guard !screens.isEmpty else { return nil }
        // Adopt the old global frame only when its display is still connected.
        if defaults.dictionary(forKey: "frames.\(id)") == nil,
           let legacy = defaults.string(forKey: "frame.\(id)") {
            let frame = NSRectFromString(legacy)
            save(frame, on: screens)
        }
        let preferred = defaults.string(forKey: "screen.\(id)")
        let screen = screens.first { $0.id == preferred }
            ?? screens.first { preferred == nil ? !$0.builtIn : $0.builtIn }
            ?? screens[0]
        let area = screen.visibleFrame
        let frames = defaults.dictionary(forKey: "frames.\(id)") as? [String: String] ?? [:]
        var frame: NSRect
        if let saved = frames[screen.id] {
            frame = NSRectFromString(saved).offsetBy(dx: screen.frame.minX, dy: screen.frame.minY)
        } else {
            let offset = CGFloat(position * 28)
            frame = NSRect(x: area.maxX - 432 - offset, y: area.maxY - 460 - offset, width: 400, height: 420)
        }
        frame.size.width = min(max(300, frame.width), area.width)
        frame.size.height = min(max(240, frame.height), area.height)
        frame.origin.x = min(max(frame.minX, area.minX), area.maxX - frame.width)
        frame.origin.y = min(max(frame.minY, area.minY), area.maxY - frame.height)
        return frame
    }

    private func screen(containing frame: NSRect, in screens: [NoteScreen]) -> NoteScreen? {
        screens.filter { $0.frame.intersects(frame) }.max {
            let left = $0.frame.intersection(frame), right = $1.frame.intersection(frame)
            return left.width * left.height < right.width * right.height
        }
    }
}
