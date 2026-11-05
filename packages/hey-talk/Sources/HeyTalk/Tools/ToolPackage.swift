import Foundation

enum ToolPackage {
    case t3, slack

    func definitions(prompt: (String) throws -> String) throws -> [[String: Any]] {
        switch self {
        case .t3: return try T3Tools.definitions(prompt: prompt)
        case .slack: return try SlackTools.definitions(prompt: prompt)
        }
    }

    func action(_ name: String, values: [String: Any]) throws -> CorrectionAction {
        switch self {
        case .t3: return try T3Tools.action(name, values: values)
        case .slack: return .slack(try SlackTools.action(name, values: values))
        }
    }

    static func tool(_ name: String, _ properties: [String: [String: Any]], prompt: (String) throws -> String) throws -> [String: Any] {
        ["type": "function", "name": name, "description": try prompt(name), "strict": true,
         "parameters": ["type": "object", "properties": properties, "required": properties.keys.sorted(), "additionalProperties": false]]
    }
}
