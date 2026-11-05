import AppKit
import XCTest
@testable import HeyTalk

final class SlackToolsTests: XCTestCase {
    func testOnlySelectedPackageIsOffered() throws {
        for package: ToolPackage? in [.slack, .t3, nil] {
            let request = try Correction.request(text: "list users", vocabulary: [], package: package)
            let json = try XCTUnwrap(JSONSerialization.jsonObject(with: request.httpBody!) as? [String: Any])
            let names = (json["tools"] as! [[String: Any]]).compactMap { $0["name"] as? String }
            XCTAssertEqual(names.filter { $0.hasPrefix("slack_") }, package == .slack ? ["slack_list_users", "slack_open_chat", "slack_open_channel"] : [])
            XCTAssertEqual(names.contains("t3_new_thread"), package == .t3)
        }
    }
    func testSpokenChannelWordsMatchSlackNames() throws {
        XCTAssertEqual(try SlackTools.action("slack_open_channel", values: ["name":" Release Notes "]), .channel("release-notes"))
        XCTAssertEqual(try SlackTools.action("slack_open_channel", values: ["name":"#build_log"]), .channel("build_log"))
    }
    func testAmbiguousNamesNeverSelectADestination() throws {
        let users = [["id":"U01", "name":"jane.s","real_name":"Jane Smith"],["id":"U02","name":"jane.t","real_name":"Jane Taylor"]]
        XCTAssertThrowsError(try SlackTools.recipients(["Jane"], users: users))
        XCTAssertThrowsError(try SlackTools.recipients(["Nobody"], users: users))
        XCTAssertEqual(try SlackTools.recipients(["Jane Smith", "@jane.t"], users: users), ["U01", "U02"])
    }
    @MainActor func testGroupAndChannelNavigationNeverPostMessages() async throws {
        var calls: [String] = []
        func api(_ method: String, _ params: [String: String]) async throws -> [String: Any] {
            calls.append(method)
            switch method {
            case "auth.test": return ["team_id":"T123"]
            case "search.modules.people":
                return ["items": params["query"] == "one" ? [["id":"U01","username":"one"]] : [["id":"U02","username":"two"]]]
            case "conversations.open":
                XCTAssertEqual(params["users"], "U02,U01")
                XCTAssertEqual(params["prevent_creation"], "true")
                return ["channel":["id":"G01"]]
            case "search.modules.channels": return ["items":[["id":"C02","name":"devops"], ["id":"C01","name":"dev"]]]
            default: XCTFail("Unexpected Slack API method: " + method); throw CancellationError()
            }
        }
        for (action, id): (SlackAction, String) in [(.chat(["two","one"]),"G01"), (.channel("dev"),"C01")] {
            guard case .chat(let url) = try await SlackTools.perform(action, team: "T123", focused: { true }, call: api) else { return XCTFail() }
            XCTAssertEqual(url.absoluteString, "slack://channel?team=T123&id=" + id)
        }
        XCTAssertEqual(calls, ["auth.test","search.modules.people","search.modules.people","conversations.open","auth.test","search.modules.channels"])
        XCTAssertThrowsError(try SlackTools.action("slack_send_message", values: ["text":"hello"]))
    }

    @MainActor func testLiveSlackNavigation() async throws {
        guard ProcessInfo.processInfo.environment["HEY_TALK_SLACK_NAV"] == "1" else { throw XCTSkip("Opt-in navigation in Slack") }
        let app = try XCTUnwrap(NSRunningApplication.runningApplications(withBundleIdentifier: "com.tinyspeck.slackmacgap").first)
        let original = try XCTUnwrap(SlackTools.route(app)), path = original.pathComponents
        let originalLink = try XCTUnwrap(FocusedInput.slackLink(original.absoluteString, threadInput: nil))
        let previous = NSWorkspace.shared.frontmostApplication, api = SlackAPI()
        let auth = try await api.call("auth.test", [:])
        let me = try XCTUnwrap(auth["user_id"] as? String)
        let profile = try await api.call("users.info", ["user": me])
        let handle = try XCTUnwrap((profile["user"] as? [String: Any])?["name"] as? String)
        let channels = try await api.call("search.modules.channels", ["module":"channels","query":"general","count":"100","page":"1"])
        let channel = try XCTUnwrap((channels["items"] as? [[String: Any]])?.first { $0["is_member"] as? Bool == true && $0["is_archived"] as? Bool != true }?["name"] as? String)
        func restore() async {
            if let bundle = app.bundleURL {
                _ = try? await NSWorkspace.shared.open([URL(string: originalLink)!], withApplicationAt: bundle, configuration: .init())
            }
            if previous != app { _ = previous?.activate() }
        }
        app.activate()
        try await Task.sleep(nanoseconds: 350_000_000)
        guard case .users(let matches) = try await SlackTools.perform(.users(handle), team: path[2], focused: { app.isActive }, call: api.call) else { return XCTFail() }
        XCTAssertFalse(matches.isEmpty)
        do {
            for action: SlackAction in [.channel(channel), .chat(["@" + handle])] {
                try await SlackTools.navigate(action, app: app, team: path[2], isCurrent: { true })
            }
        } catch { await restore(); throw error }
        await restore()
    }

    func testLiveSlackVoiceSelection() async throws {
        guard ProcessInfo.processInfo.environment["HEY_TALK_EVAL"] == "1" else { throw XCTSkip("Opt-in live model eval") }
        let correction = Correction()
        for (speech, expected): (String, SlackAction?) in [
            ("Take me to the engineering channel.", .channel("engineering")),
            ("Open my chat with Jane Smith and Lee Park.", .chat(["Jane Smith","Lee Park"])),
            ("List Slack users.", .users("")),
            ("We should add a button to open my chat with Jane Smith.", nil)
        ] {
            var actions: [CorrectionAction] = []
            let action: CorrectionAction = try await withCheckedThrowingContinuation { continuation in
                correction.correct(speech, vocabulary: [], app: { ["name":"Slack","bundle_id":"com.tinyspeck.slackmacgap"] }, package: .slack, perform: { action, next in
                    actions.append(action); next("done")
                }) { continuation.resume(with: $0) }
            }
            XCTAssertEqual(actions.count, 1)
            if let expected { XCTAssertEqual(action, .slack(expected)) }
            else if case .paste = action {} else { XCTFail("Discussion must stay dictation") }
        }
    }
}
