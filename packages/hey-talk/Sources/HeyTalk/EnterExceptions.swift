import Foundation
import TalkCore

struct EnterExceptions {
    private let defaults: UserDefaults
    init(defaults: UserDefaults = .standard) { self.defaults = defaults }
    var bundleIDs: [String] { (defaults.stringArray(forKey: "enterExceptions") ?? []).sorted() }
    var sites: [String] { (defaults.stringArray(forKey: "enterExceptionSites") ?? ["lattice.com", "latticehq.com"]).sorted() }

    func excludedSite(_ host: String) -> String? {
        let name = host.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "."))
        return sites.first { name == $0 || name.hasSuffix("." + $0) }
    }

    func shouldPressEnter(requested: Bool, bundleID: String?, host: String? = nil) -> Bool {
        requested && !bundleIDs.contains(bundleID ?? "") && host.flatMap(excludedSite) == nil
    }

    static func validate(_ arguments: [String]) throws {
        let sites = arguments.first == "sites"
        let args = sites ? Array(arguments.dropFirst()) : arguments
        guard !args.isEmpty else { return }
        let label = "[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?"
        let pattern = sites ? "^\(label)(?:\\.\(label))*\\z" : "^[A-Za-z0-9-]+(\\.[A-Za-z0-9-]+)+$"
        guard args.count == 2, ["add", "remove"].contains(args[0]), !sites || args[1].count <= 253,
              args[1].range(of: pattern, options: .regularExpression) != nil else {
            throw TalkError("Use enter-exceptions [add|remove BUNDLE_ID] or enter-exceptions sites [add|remove HOST]")
        }
    }

    func command(_ arguments: [String]) throws -> [String] {
        try Self.validate(arguments)
        let forSites = arguments.first == "sites"
        let args = forSites ? Array(arguments.dropFirst()) : arguments
        var entries = Set(forSites ? sites : bundleIDs)
        if !args.isEmpty {
            let entry = forSites ? args[1].lowercased() : args[1]
            if args[0] == "add" { entries.insert(entry) }
            else { entries.remove(entry) }
            defaults.set(entries.sorted(), forKey: forSites ? "enterExceptionSites" : "enterExceptions")
        }
        return entries.sorted()
    }
}
