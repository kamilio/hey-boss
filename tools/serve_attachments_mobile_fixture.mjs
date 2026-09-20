// Paired mobile transport fixture backed by the isolated native issue RPC.
// Seed output/playwright/issue57 first; never use a production database.
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';
import {spawn} from 'node:child_process';
import {resolve} from 'node:path';
const root=resolve('output/playwright/issue57');
const project={id:'named:Attachment Studio',name:'Attachment Studio'};
const store=new HubStore();store.setIssueProjects([project]);
const app=createApp({store,hubToken:'synthetic-fixture-credential-'.repeat(3),secure:false,origin:'http://127.0.0.1:52021'});
const server=app.listen(52021,'127.0.0.1',()=>console.log(JSON.stringify({url:'http://127.0.0.1:52021',code:store.pairing()})));
let busy=false;
const timer=setInterval(async()=>{
 if(busy)return;busy=true;
 try {
  for(const row of store.pendingArtifacts()) {
   const reading=['list','view','show','download','links','preview'].includes(row.operation.operation?.command)||row.operation.action==='view';
   const req={version:1,project,project_override:row.project,actor:reading?null:{id:'human:boss',kind:'human',session_id:null,machine:'fixture',host:'fixture',pid:null,process_start:null,cwd:root,source:'fixture'},operation:row.operation,request_id:reading?null:`mobile-fixture:${row.id}`};
   const value=await new Promise((resolvePromise,reject)=>{
    const child=spawn(resolve(root,'hey-boss-visual'),['issue','rpc'],{env:{...process.env,HEY_BOSS_ISSUE_DB:resolve(root,'issues.db')}});
    let out='';child.stdout.on('data',s=>out+=s);child.stderr.resume();child.on('error',reject);
    child.on('close',()=>{try{resolvePromise(JSON.parse(out));}catch(e){reject(e);}});child.stdin.end(JSON.stringify(req));
   });
   store.finishArtifact(row.id,value);
  }
 } catch(e){console.error(e.message);} finally{busy=false;}
},100);
function close(){clearInterval(timer);app.locals.close();server.close(()=>{store.close();process.exit(0);});}
process.on('SIGTERM',close);process.on('SIGINT',close);
