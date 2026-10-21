import Foundation
import TalkCore

struct SpeechSettings {
    private let defaults: UserDefaults
    init(defaults: UserDefaults = .standard) { self.defaults = defaults }
    var enabled: Bool { defaults.object(forKey: "speechEnabled") as? Bool ?? true }
    var bundleIDs: [String] { (defaults.stringArray(forKey: "speechApps") ?? ["com.t3tools.t3code"]).sorted() }
    func allows(_ bundleID: String) -> Bool { enabled && bundleIDs.contains(bundleID) }
    var json: [String: Any] { ["enabled": enabled, "apps": bundleIDs] }
    static func validate(_ args: [String]) throws {
        if args.isEmpty || (args.count == 1 && ["on", "off", "target", "play", "pause", "stop"].contains(args[0])) { return }
        guard args.count == 2, ["add", "remove"].contains(args[0]),
              args[1].range(of: "^[A-Za-z0-9-]+(\\.[A-Za-z0-9-]+)+$", options: .regularExpression) != nil else {
            throw TalkError("Use speech [on|off|add BUNDLE_ID|remove BUNDLE_ID|target|play|pause|stop]")
        }
    }
    func command(_ args: [String]) throws {
        try Self.validate(args)
        guard let action = args.first else { return }
        if action == "on" || action == "off" { defaults.set(action == "on", forKey: "speechEnabled") }
        if action == "add" || action == "remove" {
            var apps = Set(bundleIDs)
            if action == "add" { apps.insert(args[1]) } else { apps.remove(args[1]) }
            defaults.set(apps.sorted(), forKey: "speechApps")
        }
    }
}

