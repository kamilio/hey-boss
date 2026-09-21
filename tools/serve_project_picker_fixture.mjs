// Isolated native and paired-device project picker; no real Inbox or agents.
// First run cargo build and npm --prefix mobile run build.
import {mkdirSync,mkdtempSync,rmSync,copyFileSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';
const output=resolve('output/playwright/issue87');mkdirSync(output,{recursive:true});
const root=mkdtempSync(output+'/runtime-'),binary=root+'/hey-boss';
copyFileSync(resolve('target/debug/hey-boss'),binary);
const env={...process.env,HEY_BOSS_ISSUE_DB:root+'/issues.db',HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:root+'/inbox.sock'};
for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_AGENT_ID','CODEX_THREAD_ID'])delete env[key];
env.HEY_BOSS_AGENT_ID='human:qa';
const projects=Array.from({length:20},(_,i)=>({id:'named:Project '+String(i+1).padStart(2,'0'),name:'Project '+String(i+1).padStart(2,'0')}));
projects.push({id:'named:A very long project name for mobile layout testing',name:'A very long project name for mobile layout testing'});
const cli=args=>{const result=spawnSync(binary,['issue',...args],{env,encoding:'utf8'});if(result.status)throw Error(result.stdout+result.stderr);return result.stdout;};
for(const project of projects)cli(['--project',project.id,'create','--title','Review the mobile project menu']);
cli(['--project','Hidden project','create','--title','Hidden fixture']);
cli(['--project','Hidden project','hide-project']);
const web=spawn(binary,['issue','--project',projects[0].id,'web','--port','59687','--no-discovery','--json'],{env,stdio:['ignore','inherit','inherit']});
const store=new HubStore();store.setIssueProjects(projects);
const key='synthetic-project-picker-fixture-'.repeat(3),base='http://127.0.0.1:52087',native='http://127.0.0.1:59687';
const app=createApp({store,hubToken:key,secure:false,origin:base});
app.get('/fixture/pairing',(_req,res)=>res.json({code:store.pairing()}));
const server=app.listen(52087,'127.0.0.1');
const bridge=async(path,body)=>{
 const response=await fetch(base+path,{method:body?'POST':'GET',headers:{Authorization:'Bearer '+key,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined});
 if(!response.ok)throw Error(await response.text());return response.json();
};
let syncing=false,boot;
const timer=setInterval(async()=>{
 if(syncing)return;syncing=true;
 try {
  boot ||= await (await fetch(native+'/api/bootstrap')).json();
  for(const row of store.pendingArtifacts()){
   const result=spawnSync(binary,['issue','rpc'],{env,encoding:'utf8',input:JSON.stringify({version:1,project:projects[0],project_override:row.project,actor:null,operation:row.operation,request_id:null})});
   store.finishArtifact(row.id,JSON.parse(result.stdout));
  }
  for(const request of (await bridge('/api/bridge/web')).requests){
   const result=request.kind==='bootstrap'?boot:request.kind==='inbox'?{ok:true,tasks:[]}:
    await (await fetch(native+'/api/'+(request.kind==='action'?'action':'preview'),{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf},body:JSON.stringify(request.payload)})).json();
   await bridge('/api/bridge/web/'+request.id+'/result',result);
  }
  await bridge('/api/bridge/agents/status',{ok:true,machines:[],signals:[],conflicts:[]});
 }catch(error){if(!closing)console.error(error.message);}finally{syncing=false;}
},100);
let closing=false;
function close(){if(closing)return;closing=true;clearInterval(timer);app.locals.close();web.kill();server.closeAllConnections();server.close(()=>store.close());}
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,close);
web.on('exit',()=>{close();rmSync(root,{recursive:true,force:true});});
console.log(JSON.stringify({fixture_pid:process.pid,native:'http://127.0.0.1:59687',paired:'http://127.0.0.1:52087'}));
