import Foundation
func auditMobile() {
    do {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("hey-boss-mobile-audit-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let portFile = root.appendingPathComponent("port")
        let server = Process(); server.executableURL = URL(fileURLWithPath: "/usr/bin/env"); server.arguments = ["node", "mobile/server/native-fixture.mjs", portFile.path]
        server.standardOutput = FileHandle.nullDevice; server.standardError = FileHandle.nullDevice
        try server.run(); defer { if server.isRunning { server.terminate(); server.waitUntilExit() } }
        let deadline = Date().addingTimeInterval(5)
        while !FileManager.default.fileExists(atPath: portFile.path), Date() < deadline { Thread.sleep(forTimeInterval: 0.02) }
        let port = try String(contentsOf: portFile, encoding: .utf8)
        let store = try Store(root.appendingPathComponent("history.db").path)
        let hub = try MobileHub(store: store, configuration: .init(url: "http://127.0.0.1:" + port, token: "native-audit-local-token-000000000000")); store.mobile = hub
        func row(_ id: String) throws -> Record {
            let data = try JSONSerialization.data(withJSONObject: ["taskID":id,"kind":"approval","question":"Ship it?","project":"Fixture","title":"Approval race","description":"Exactly one answer","options":["Approve","Reject"],"createdAt":Date().timeIntervalSince1970,"status":"pending"])
            return try JSONDecoder().decode(Record.self, from: data)
        }
        let local = try row("local-first"); try store.database.save(local);try hub.track(local)
        try store.finish(local.taskID, "Approve")
        let localResult = try store.database.get(local.taskID); precondition(localResult.result == "Approve")
        let (status, result) = try hub.call("/api/bridge/tasks/"+local.taskID+"/resolve", method:"POST", body:["result":"Reject"])
        precondition(status == 409 && (result["task"] as? [String:Any])?["result"] as? String == "Approve")
        let remote = try row("phone-first");try store.database.save(remote);try hub.track(remote);_ = try hub.publish(remote)
        _ = try hub.call("/test/phone/"+remote.taskID, method:"POST", body:["result":"Reject"])
        try store.finish(remote.taskID, "Approve")
        let remoteResult = try store.database.get(remote.taskID); precondition(remoteResult.result == "Reject")
        let (_, requestCounts) = try hub.call("/test/counts")
        let counts = requestCounts["counts"] as! [String: Any]
        precondition(counts["PUT /api/bridge/tasks/phone-first"] as? Int == 1)
        precondition(counts["POST /api/bridge/tasks/phone-first/resolve"] as? Int == 1)
        print("Passed: existing notification resolution uses one request without republishing; unknown task publication fallback")
        let syncing = try row("phone-sync");try store.database.save(syncing);try hub.track(syncing);_ = try hub.publish(syncing)
        _ = try hub.call("/test/phone/"+syncing.taskID, method:"POST", body:["result":"Approve"])
        hub.sync();hub.sync();let syncResult = try store.database.get(syncing.taskID);precondition(syncResult.result == "Approve")
        let markdown = "# Full server report\n\n" + String(repeating: "**Multiline** update from devbox.\n", count: 1000)
        let updateData = try JSONSerialization.data(withJSONObject: ["taskID":"phone-open-update","kind":"update","question":markdown,"project":"Fixture","title":"Read receipt","description":"Read the report","options":[],"createdAt":Date().timeIntervalSince1970,"status":"pending"])
        let update = try JSONDecoder().decode(Record.self, from:updateData)
        try store.database.save(update);try hub.track(update);_ = try hub.publish(update)
        let (_, document) = try hub.call("/test/document/"+update.taskID)
        precondition((document["task"] as? [String:Any])?["question"] as? String == markdown)
        _ = try hub.call("/test/open/"+update.taskID,method:"POST",body:[:])
        hub.sync();hub.sync();let openedUpdate = try store.database.get(update.taskID);precondition(openedUpdate.status == "ok")
        let (pollStatus, poll) = try hub.call("/api/bridge/tasks");precondition(pollStatus == 200 && (poll["tasks"] as? [[String:Any]])?.isEmpty == true)
        let clearIDs=(0..<105).map { "clear-\($0)" }
        let clearRows=try clearIDs.map { try row($0) }
        for item in clearRows { try store.database.save(item); _ = try hub.publish(item) }
        _ = try hub.call("/test/phone/"+clearIDs[0],method:"POST",body:["result":"Approve"])
        try hub.clear(clearRows)
        for id in clearIDs {
            let item=try store.database.get(id)
            precondition(id==clearIDs[0] ? item.result=="Approve" && item.status=="ok" : item.status=="cancelled" && item.result==nil)
        }
        let (_,clearCounts)=try hub.call("/test/counts")
        precondition((clearCounts["counts"] as! [String:Any])["POST /api/bridge/tasks/clear"] as? Int==2)
        let unpublished=try row("clear-unpublished");try store.database.save(unpublished);try hub.clear([unpublished])
        let unpublishedResult=try store.database.get(unpublished.taskID);precondition(unpublishedResult.status=="cancelled")
        print("Passed: native bulk clear uses bounded batches, preserves phone winners, cancels without approval, and publishes unknown tasks")
        let offline = try row("offline");try store.database.save(offline);server.terminate();server.waitUntilExit()
        do { try hub.clear([offline]);throw StorageError(description:"Offline clear was incorrectly accepted") }
        catch { precondition((try! store.database.get(offline.taskID)).status=="pending") }
        do { try store.finish(offline.taskID, "Approve");throw StorageError(description:"Offline answer was incorrectly accepted") }
        catch { let offlineResult = try store.database.get(offline.taskID);precondition(offlineResult.status == "pending") }
        store.mobile = nil; store.mobileRequired = true
        let invalid = try row("invalid-config"); try store.database.save(invalid)
        do { try store.finish(invalid.taskID, "Approve"); throw StorageError(description: "Invalid configuration accepted an answer") }
        catch { let retained = try store.database.get(invalid.taskID); precondition(retained.status == "pending") }
        print("Passed: invalid configuration refuses answers; native Mac-first and phone-first races, exact winning result, durable mobile delivery/ack, repeat sync, full Markdown transfer, phone opens clear Mac updates, offline answer remains pending")
    } catch { fputs("Mobile audit failed: \(error)\n", stderr); exit(1) }
}
