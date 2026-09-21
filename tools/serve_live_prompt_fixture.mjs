// Isolated desktop/paired-device project settings review. No agents are launched.
import {mkdirSync,mkdtempSync,rmSync,copyFileSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';
const output=resolve('output/playwright/issue83');mkdirSync(output,{recursive:true});
const root=mkdtempSync(output+'/runtime-'),binary=root+'/hey-boss';
copyFileSync(resolve('target/debug/hey-boss'),binary);
const project={id:'named:Prompt Studio',name:'Prompt Studio'};
const env={...process.env,HEY_BOSS_ISSUE_DB:root+'/issues.db',HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:root+'/absent.sock'};
for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_AGENT_ID','CODEX_THREAD_ID'])delete env[key];
env.HEY_BOSS_AGENT_ID='human:qa';
const cli=args=>{
 const result=spawnSync(binary,['issue','--json','--project',project.id,'--agent','human:qa',...args],{env,cwd:root,encoding:'utf8'});
 if(result.status)throw Error(result.stdout+result.stderr);return JSON.parse(result.stdout);
};
cli(['create','--title','Keep running agents up to date','--body','Save instructions without restarting active work.']);
const web=spawn(binary,['issue','--project',project.id,'web','--port','4883','--no-discovery','--json'],{env,cwd:root,stdio:['ignore','inherit','inherit']});
const store=new HubStore();store.setIssueProjects([project]);
const key='synthetic-prompt-fixture-'.repeat(4),base='http://127.0.0.1:5283',native='http://127.0.0.1:4883';
const app=createApp({store,hubToken:key,secure:false,origin:base});
app.get('/fixture/pairing',(_req,res)=>res.json({code:store.pairing()}));
const server=app.listen(5283,'127.0.0.1');
const bridge=async(path,body)=>{
 const response=await fetch(base+path,{method:body?'POST':'GET',headers:{Authorization:'Bearer '+key,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined});
 if(!response.ok)throw Error(await response.text());return response.json();
};
let busy=false;
const timer=setInterval(async()=>{
 if(busy)return;busy=true;
 try{
  const boot=await (await fetch(native+'/api/bootstrap')).json();
  for(const request of (await bridge('/api/bridge/web')).requests){
   const result=request.kind==='bootstrap'?boot:request.kind==='inbox'?{ok:true,tasks:[]}:
    await (await fetch(native+'/api/'+(request.kind==='action'?'action':'preview'),{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf},body:JSON.stringify(request.payload)})).json();
   await bridge('/api/bridge/web/'+request.id+'/result',result);
  }
  await bridge('/api/bridge/agents/status',{ok:true,machines:[],signals:[],conflicts:[]});
 }catch(error){if(!closing)console.error(error.message);}finally{busy=false;}
},100);
let closing=false;
function close(){if(closing)return;closing=true;clearInterval(timer);app.locals.close();web.kill('SIGTERM');server.closeAllConnections();server.close(()=>store.close());}
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,close);
web.on('exit',()=>{close();rmSync(root,{recursive:true,force:true});});
console.log(JSON.stringify({fixture_pid:process.pid,native,paired:base}));
