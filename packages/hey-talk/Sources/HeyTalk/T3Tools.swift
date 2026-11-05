import Foundation
import TalkCore

enum T3Tools {
    static func definitions(prompt: (String) throws -> String) throws -> [[String: Any]] {
        func tool(_ name: String, _ properties: [String: [String: Any]] = [:]) throws -> [String: Any] {
            ["type": "function", "name": name, "description": try prompt(name), "strict": true,
             "parameters": ["type": "object", "properties": properties, "required": properties.keys.sorted(), "additionalProperties": false]]
        }
        return try [
            tool("t3_rename_thread", ["title": ["type": "string"]]),
            tool("t3_new_thread", ["project": ["type": "string"], "text": ["type": "string"]]),
            tool("t3_list_threads"),
            tool("t3_switch_thread", ["position": ["type": "integer", "minimum": 1, "maximum": 9]]),
            tool("t3_settle_thread"), tool("t3_pin_thread", ["pinned": ["type": "boolean"]])
        ]
    }

    static func action(_ name: String, values: [String: Any]) throws -> CorrectionAction {
        switch name {
        case "t3_rename_thread" where values.count == 1:
            guard let title = (values["title"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines), !title.isEmpty,
                  title.rangeOfCharacter(from: .controlCharacters.union(.newlines)) == nil else { throw TalkError("Invalid thread title.") }
            return .rename(title)
        case "t3_new_thread" where values.count == 2:
            guard let project = (values["project"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
                  project.rangeOfCharacter(from: .controlCharacters) == nil,
                  let text = (values["text"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines) else { throw TalkError("Invalid new thread request.") }
            return .newThread(project: project.isEmpty ? nil : project, text: text)
        case "t3_list_threads" where values.isEmpty: return .listThreads
        case "t3_switch_thread" where values.count == 1:
            guard let number = values["position"] as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID(),
                  let position = values["position"] as? Int, (1...9).contains(position) else { throw TalkError("Choose a T3 thread from 1 to 9.") }
            return .switchThread(position)
        case "t3_settle_thread" where values.isEmpty: return .settleThread
        case "t3_pin_thread" where values.count == 1:
            guard let pinned = values["pinned"] as? NSNumber, CFGetTypeID(pinned) == CFBooleanGetTypeID() else { throw TalkError("Choose whether to pin or unpin the T3 thread.") }
            return .pinThread(pinned.boolValue)
        default: throw TalkError("Invalid T3 tool call.")
        }
    }
}
