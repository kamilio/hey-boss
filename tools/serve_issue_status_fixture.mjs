// Isolated desktop and paired-device status review; no agents or fleet services.
import {mkdirSync,rmSync,copyFileSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';
const root=resolve('output/playwright/issue70');mkdirSync(root,{recursive:true});
for(const suffix of ['', '-wal', '-shm'])rmSync(root+'/issues.db'+suffix,{force:true});
console.log(JSON.stringify({fixture_pid:process.pid}));
const binary=resolve(root,'hey-boss-visual');copyFileSync(resolve('target/debug/hey-boss'),binary);
const project={id:'named:Progress Studio',name:'Progress Studio'};
const env={...process.env,HEY_BOSS_ISSUE_DB:root+'/issues.db',HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:root+'/inbox.sock'};
delete env.HEY_BOSS_ISSUE_HOST;delete env.HEY_BOSS_ISSUE_PROJECT;
const cli=(args)=>{const r=spawnSync(binary,['issue','--json','--project',project.id,'--agent','human:qa',...args],{env,encoding:'utf8'});if(r.status)throw Error(r.stdout+r.stderr);return JSON.parse(r.stdout);};
for(const [title,status,message] of [
 ['Reconnect after waking from sleep','green','The fix passes tests. Checking the phone layout next.'],
 ['Install on the connected devices','orange','One device is offline. Retrying the installation.'],
 ['Restore the deployment connection','red','The server is unavailable. Waiting for the connection to return.'],
 ['Plan next week’s improvements',null,null],
 ['Review a long update','orange','A'.repeat(500)],
 ['Keep status text safe','green','<img src=x onerror=alert(1)> & "quoted" status stays plain text.'],
]){
 const number=cli(['create','--title',title,'--body','## What needs to happen\n\nKeep the work easy to follow, with clear updates and lasting findings in comments.','--label','enhancement']).issue.number;
 if(status){cli(['claim',String(number)]);if(number===1)for(let i=0;i<24;i++)cli(['status','1',i%3===0?'orange':'green','--comment',`Checking the reconnect behavior, step ${i+1}.`]);cli(['status',String(number),status,'--comment',message]);}
}
const web=spawn(binary,['issue','--project',project.id,'web','--port','4796','--no-discovery','--json'],{env,stdio:['ignore','inherit','inherit']});
const store=new HubStore();store.setIssueProjects([project]);
const app=createApp({store,hubToken:'synthetic-status-fixture-'.repeat(4),secure:false,origin:'http://127.0.0.1:52070'});
app.get('/fixture/pairing',(_req,res)=>res.json({code:store.pairing()}));
app.get('/fixture/history',(_req,res)=>res.json(cli(['status-history','1','--limit','100'])));
let burst=0;
app.post('/fixture/status',(_req,res)=>{
 let result;burst++;
 for(let step=1;step<=3;step++)result=cli(['status','1','green','--comment',`A new update arrived while reading history, check ${burst}, step ${step}.`]);
 res.json(result);
});
const server=app.listen(52070,'127.0.0.1',()=>console.log(JSON.stringify({mobile:'http://127.0.0.1:52070',code:store.pairing()})));
let busy=false;
const timer=setInterval(()=>{
 if(busy)return;busy=true;
 try {for(const row of store.pendingArtifacts()){
  const request={version:1,project,project_override:row.project,actor:null,operation:row.operation,request_id:null};
  const result=spawnSync(binary,['issue','rpc'],{env,encoding:'utf8',input:JSON.stringify(request)});
  store.finishArtifact(row.id,JSON.parse(result.stdout));
 }}catch(e){console.error(e.message);}finally{busy=false;}
},100);
let closing=false;
function close(){if(closing)return;closing=true;clearInterval(timer);app.locals.close();web.kill('SIGTERM');server.close(()=>{store.close();});}
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,close);
web.on('exit',()=>{close();rmSync(root+'/hey-boss-visual',{force:true});});
