import Foundation
import TalkCore

let help = """
hey-clipboard — local clipboard history

  status                       Counts, pause state, shortcut, and persistence status
  list [--pinned]               Recent entries with stable IDs and previews
  search QUERY                 Search full text and source app
  get ID [--raw]                Read one complete entry (raw prints just its text)
  copy ID                      Put an entry on the clipboard; move it to the top
  copy --stdin                 Copy UTF-8 input and save it in history
  paste ID --app BUNDLE_ID      Activate that app and paste; never send Enter
  delete ID                    Delete one history entry
  pin ID | unpin ID             Keep an entry beyond the 40-entry history limit
  clear                        Delete all history, including pins
  pause | resume               Pause or resume capture
  show                         Open the Liquid Glass picker (also Command+Shift+V)
  hide                         Close the picker
  import-flycut                 Import Flycut's saved history and favorites
  quit                         Save and stop the app

All commands return JSON, except get --raw. Errors return {ok:false,error:...}
and exit 1. Use IDs from list/search. Text is local to this Mac; temporary,
concealed, and generated clipboard entries are excluded. Paste uses Hey Talk's
existing Accessibility permission. Hyper+V remains Hey Talk's dictation recovery.
Hold Command+Shift, tap V to cycle, then release to paste. Escape cancels.
"""

func runCLI(_ arguments: [String]) throws -> Int32 {
    if arguments == ["--help"] || arguments == ["help"] { print(help); return 0 }
    guard let command = arguments.first else { throw TalkError("Missing command") }
    var request: [String: Any] = ["command": command]
    var raw = false
    switch command {
    case "status", "show", "hide", "pause", "resume", "quit", "clear", "import-flycut":
        guard arguments.count == 1 else { throw TalkError("Unexpected argument") }
    case "list":
        guard arguments.count == 1 || arguments == ["list", "--pinned"] else { throw TalkError("Usage: list [--pinned]") }
        request["pinned"] = arguments.count == 2
    case "search":
        guard arguments.count >= 2 else { throw TalkError("Usage: search QUERY") }
        request["query"] = arguments.dropFirst().joined(separator: " ")
    case "get", "copy", "delete", "pin", "unpin":
        if command == "copy", arguments == ["copy", "--stdin"] {
            let data = FileHandle.standardInput.readDataToEndOfFile()
            guard data.count <= 1_048_576, let text = String(data: data, encoding: .utf8), !text.isEmpty else { throw TalkError("Expected nonempty UTF-8 text, at most 1 MiB") }
            request["text"] = text
        } else {
            raw = command == "get" && arguments.count == 3 && arguments[2] == "--raw"
            guard arguments.count == (raw ? 3 : 2), UUID(uuidString: arguments[1]) != nil else { throw TalkError("Expected an entry ID from list or search") }
            request["id"] = arguments[1]
        }
    case "paste":
        guard arguments.count == 4, UUID(uuidString: arguments[1]) != nil, arguments[2] == "--app", !arguments[3].isEmpty else { throw TalkError("Usage: paste ID --app BUNDLE_ID") }
        request["id"] = arguments[1]; request["app"] = arguments[3]
    default: throw TalkError("Unknown command. Run hey-clipboard --help.")
    }
    let result = try Server.request(request)
    if raw, result["ok"] as? Bool == true, let entry = result["entry"] as? [String: Any], let text = entry["text"] as? String {
        FileHandle.standardOutput.write(Data(text.utf8))
    } else { FileHandle.standardOutput.write(try Wire.json(result)) }
    return result["ok"] as? Bool == true ? 0 : 1
}
