import Foundation
import TalkCore

/// Per-app exceptions can suppress Enter, but never turn paste-only into submit.
struct EnterExceptions {
    private let defaults: UserDefaults
    init(defaults: UserDefaults = .standard) { self.defaults = defaults }
    var bundleIDs: [String] { (defaults.stringArray(forKey: "enterExceptions") ?? []).sorted() }

    func shouldPressEnter(requested: Bool, bundleID: String?) -> Bool {
        requested && !bundleIDs.contains(bundleID ?? "")
    }

    static func validate(_ arguments: [String]) throws {
        guard !arguments.isEmpty else { return }
        guard arguments.count == 2, ["add", "remove"].contains(arguments[0]),
              arguments[1].range(of: "^[A-Za-z0-9-]+(\\.[A-Za-z0-9-]+)+$", options: .regularExpression) != nil else {
            throw TalkError("Use enter-exceptions [add|remove BUNDLE_ID], e.g. enter-exceptions add com.tinyspeck.slackmacgap")
        }
    }

    func command(_ arguments: [String]) throws -> [String] {
        try Self.validate(arguments)
        if !arguments.isEmpty {
            var ids = Set(bundleIDs)
            if arguments[0] == "add" { ids.insert(arguments[1]) }
            else { ids.remove(arguments[1]) }
            defaults.set(ids.sorted(), forKey: "enterExceptions")
        }
        return bundleIDs
    }
}
