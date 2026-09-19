import Foundation
import Darwin

/// Isolated real native store for browser and HTTP checks. No banners or LaunchAgent.
func runInboxFixture() {
    signal(SIGPIPE,SIG_IGN)
    guard CommandLine.arguments.count == 2 else { fatalError("Expected fixture directory") }
    let directory=URL(fileURLWithPath:CommandLine.arguments[1])
    try! FileManager.default.createDirectory(at:directory,withIntermediateDirectories:true)
    let path=directory.appendingPathComponent("daemon.sock").path
    precondition(path.utf8.count<104)
    let store=try! Store(directory.appendingPathComponent("history.db").path)
    let rows:[(String,String,String)] = [
        ("update","Build ready","# Build report\n\n**Checks passed.**\n\n| Suite | Result |\n|---|---|\n| Native | Passed |\n\n<script>window.injected=true</script>"),
        ("approval","Publish release?","Publish the release?"),
        ("prompt","Choose a direction","Which approach should we use?"),
        ("update","Review implementation","# Review\n\nPlease review this implementation."),
        ("alert","Deployment complete","Deployment complete."),
        ("update","Earlier report","# Previous report")]
    if try! store.database.inboxRows().isEmpty {
        for (index,item) in rows.enumerated() {
            var row=Record(taskID:"notice-\(index+1)",kind:item.0,question:item.2,project:index==4 ? "Other project" : "Inbox QA",title:item.1,description:item.0=="update" ? "Report ready for review." : "Your input is welcome.",options:item.0=="approval" ? ["Approve","Reject"] : [],autoclose:nil,linkURL:nil,linkLabel:nil,createdAt:Date().timeIntervalSince1970-Double(index*60),presentedAt:nil,expiresAt:nil,status:index==5 ? "ok" : "pending",result:nil,origin:nil)
            row.sourceHost="This Mac";row.sourceKnown=true;row.commentsEnabled=index==3;row.severity=index==4 ? "success" : "info"
            if index==5 { row.completedAt=Date().timeIntervalSince1970-120 }
            try! store.database.save(row)
        }
    }
    let listener=socket(AF_UNIX,SOCK_STREAM,0);precondition(listener>=0)
    var address=sockaddr_un();address.sun_family=sa_family_t(AF_UNIX)
    withUnsafeMutableBytes(of:&address.sun_path) { buffer in for (i,b) in (Array(path.utf8)+[0]).enumerated() { buffer[i]=b } }
    let bound=withUnsafePointer(to:&address) { pointer in pointer.withMemoryRebound(to:sockaddr.self,capacity:1) { Darwin.bind(listener,$0,socklen_t(MemoryLayout<sockaddr_un>.size)) } }
    precondition(bound==0 && listen(listener,16)==0,"Could not bind isolated Inbox socket")
    print("Inbox fixture ready: \(path)");fflush(stdout)
    while true {
        let fd=accept(listener,nil,nil);if fd<0 { if errno==EINTR { continue };return }
        DispatchQueue.global().async {
            let reply=Reply(fd)
            do { let data=try readRequest(fd);let request=try JSONDecoder().decode(Request.self,from:data);store.queue.async { store.handle(request,reply) } }
            catch { reply.send(["status":"error","error":"Invalid fixture request"]) }
        }
    }
}
