import Foundation
import TalkCore

enum ToolPackage {
    case t3, visibleT3Thread, slack

    func definitions(prompt: (String) throws -> String) throws -> [[String: Any]] {
        switch self {
        case .t3: return try T3Tools.definitions(prompt: prompt)
        case .visibleT3Thread: return try T3Tools.definitions(prompt: prompt).filter { Self.visibleThreadTools.contains($0["name"] as? String ?? "") }
        case .slack: return try SlackTools.definitions(prompt: prompt)
        }
    }

    func action(_ name: String, values: [String: Any]) throws -> CorrectionAction {
        switch self {
        case .visibleT3Thread where !Self.visibleThreadTools.contains(name): throw TalkError("Unavailable thread tool.")
        case .t3, .visibleT3Thread: return try T3Tools.action(name, values: values)
        case .slack: return .slack(try SlackTools.action(name, values: values))
        }
    }

    private static let visibleThreadTools = ["t3_pin_thread", "t3_unpin_thread", "t3_rename_thread", "t3_settle_thread"]

    static func tool(_ name: String, _ properties: [String: [String: Any]], prompt: (String) throws -> String) throws -> [String: Any] {
        ["type": "function", "name": name, "description": try prompt(name), "strict": true,
         "parameters": ["type": "object", "properties": properties, "required": properties.keys.sorted(), "additionalProperties": false]]
    }
}
