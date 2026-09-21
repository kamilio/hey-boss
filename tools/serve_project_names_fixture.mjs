// Isolated project-name QA. Pass a CLI binary as the first argument if needed.
import {mkdirSync,mkdtempSync,rmSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';
const output=resolve('output/playwright/issue107');mkdirSync(output,{recursive:true});
const root=mkdtempSync(output+'/runtime-'),binary=resolve(process.argv[2]||'target/debug/hey-boss');
const env={...process.env,HEY_BOSS_ISSUE_DB:root+'/issues.db',HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:root+'/inbox.sock',HEY_BOSS_AGENT_ID:'human:qa'};
for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID'])delete env[key];
const cli=args=>{const result=spawnSync(binary,['issue','--json',...args],{env,encoding:'utf8'});if(result.status)throw Error(result.stdout+result.stderr);return JSON.parse(result.stdout);};
cli(['--project','github.com/poe-internal/poe2','create','--title','Original destination']);
cli(['--project','github.com/other/poe2','create','--title','Reused destination']);
cli(['--project','named:poe2','create','--title','Named alias']);
for(const name of ['Design Team','A very long project name for narrow phone layouts','Hidden project'])cli(['--project',name,'create','--title','Fixture']);
cli(['--project','Hidden project','hide-project']);
const web=spawn(binary,['issue','--project','poe2','web','--port','59692','--no-discovery','--json'],{env,stdio:['ignore','inherit','inherit']});
const store=new HubStore(),key='synthetic-project-name-fixture-'.repeat(3),base='http://127.0.0.1:52092',native='http://127.0.0.1:59692';
const app=createApp({store,hubToken:key,secure:false,origin:base});
app.get('/fixture/pairing',(_req,res)=>res.json({code:store.pairing()}));
const server=app.listen(52092,'127.0.0.1');
const bridge=async(path,body)=>{const r=await fetch(base+path,{method:body?'POST':'GET',headers:{Authorization:'Bearer '+key,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined});if(!r.ok)throw Error(await r.text());return r.json();};
let syncing=false,closing=false;
const timer=setInterval(async()=>{
 if(syncing)return;syncing=true;
 try{
  const boot=await(await fetch(native+'/api/bootstrap')).json();
  store.setIssueProjects(boot.projects.filter(p=>!p.hidden_at));
  for(const request of (await bridge('/api/bridge/web')).requests){
   const result=request.kind==='bootstrap'?boot:request.kind==='inbox'?{ok:true,tasks:[]}:await(await fetch(native+'/api/'+(request.kind==='action'?'action':'preview'),{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf},body:JSON.stringify(request.payload)})).json();
   await bridge('/api/bridge/web/'+request.id+'/result',result);
  }
  for(const creation of store.pendingIssues()){
   const result=cli(['--project',creation.project,'--agent','human:boss','--request-id','mobile:'+creation.requestID,'create','--title',creation.title]);
   store.finishIssue(creation.requestID,{status:'synced',number:result.issue.number});
  }
  await bridge('/api/bridge/agents/status',{ok:true,machines:[],signals:[],conflicts:[]});
 }catch(error){if(!closing)console.error(error.message);}finally{syncing=false;}
},150);
function close(){if(closing)return;closing=true;clearInterval(timer);app.locals.close();web.kill();server.closeAllConnections();server.close(()=>store.close());}
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,close);
web.on('exit',()=>{close();rmSync(root,{recursive:true,force:true});});
console.log(JSON.stringify({fixture_pid:process.pid,native,paired:base}));
