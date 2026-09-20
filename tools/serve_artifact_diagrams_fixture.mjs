// Isolated production native/mobile servers for Mermaid browser checks.
import {mkdtempSync,rmSync,copyFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawn,execFileSync} from 'node:child_process';
import {createInterface} from 'node:readline';
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';

const root=mkdtempSync(join(tmpdir(),'hey-boss-diagrams-'));
// A build in the shared checkout must not auto-reload this server mid-test.
const binary=join(root,'hey-boss');
copyFileSync(resolve(process.argv[2]||'target/debug/hey-boss'),binary);
const env={...process.env,HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_INBOX_SOCKET:join(root,'inbox.sock')};
delete env.HEY_BOSS_ISSUE_HOST;
const source='# Recovery flow\n\nFollow the saved request through each service. Expand the diagram to see the full route.\n\n```mermaid\nflowchart LR\n user[You send a message] --> chat[Chat saves your message]\n chat --> observer[Reply tracker saves a delivery task]\n observer --> session[Session saves agent history]\n session --> runner[Runner coordinates work]\n runner --> model[Model service calls provider]\n runner --> tools[Tool service performs actions]\n model --> saved[Saved model and tool results]\n tools --> saved\n saved --> session\n session --> observer\n observer --> outbox[Delivery queue keeps reply until confirmed]\n outbox --> chat\n chat --> user\n```\n\n## Recovery decision\n\n```mermaid\nflowchart TD\n A[Request interrupted] --> B{Result saved?}\n B -->|Yes| C[Replay the same result]\n B -->|No| D[Report incomplete work]\n C --> E[Confirm delivery]\n D --> E\n```\n\n## Broken source stays available\n\n```mermaid\nflowchart LR\n A[Broken\n```\n\n## Ordinary code\n\n```js\nconst saved = true;\n```\n\n## Untrusted labels\n\n```mermaid\nflowchart LR\n A["<script>window.diagramPwned=1</script>"] --> B[Safe]\n click B "javascript:window.diagramPwned=2"\n```';
execFileSync(binary,['artifact','create','--project','Flowchart QA','--title','Recovery flow','--body',source],{env});
const child=spawn(binary,['issue','web','--project','Flowchart QA','--no-discovery','--port','59549','--json'],{env,stdio:['ignore','pipe','inherit']});
const lines=createInterface({input:child.stdout});
const ready=await new Promise((resolve,reject)=>{lines.once('line',line=>resolve(JSON.parse(line)));child.once('exit',code=>reject(Error('Native server exited '+code)));});
const native=ready.url.replace(/\/$/,''),base='http://127.0.0.1:59550';
const boot=await(await fetch(native+'/api/bootstrap')).json();
const store=new HubStore();store.setIssueProjects(boot.projects);
const app=createApp({store,hubToken:'synthetic-diagram-fixture-token'.repeat(2),origin:base,secure:false});
app.get('/fixture-pairing',(req,res)=>res.json({code:store.pairing()}));
const server=app.listen(59550,'127.0.0.1');
let syncing=false;
const timer=setInterval(async()=>{
 if(syncing)return;syncing=true;
 try{for(const request of store.pendingArtifacts()){
  const current=await(await fetch(native+'/api/bootstrap')).json();
  store.setIssueProjects(current.projects);
  const read=['list','view','links','preview'].includes(request.operation.operation?.command);
  const response=await fetch(native+'/api/action',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':current.csrf},body:JSON.stringify({project:request.project,operation:request.operation,request_id:read?null:request.id})});
  store.finishArtifact(request.id,await response.json());
 }}catch(e){console.error(e.message);}finally{syncing=false;}
},100);
console.log(JSON.stringify({native,mobile:base,project:'named:Flowchart QA'}));
let closing=false;
function close(){
 if(closing)return;closing=true;clearInterval(timer);child.kill('SIGTERM');app.locals.close();server.closeAllConnections();
 server.close(()=>{store.close();child.once('exit',()=>{rmSync(root,{recursive:true,force:true});process.exit(0);});if(child.exitCode!==null){rmSync(root,{recursive:true,force:true});process.exit(0);}});
}
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,close);
