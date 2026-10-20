// Isolated production desktop/mobile servers; never touches the user's issues.
import {mkdtempSync,copyFileSync,readFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawn,execFileSync} from 'node:child_process';
import {createInterface} from 'node:readline';
import {createServer} from 'node:http';
import {createServer as createSocket} from 'node:net';
import {fileURLToPath} from 'node:url';
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';

export async function startFixture(){
 const root=mkdtempSync(join(tmpdir(),'hb-html-'));
 const binary=join(root,'hey-boss');copyFileSync(resolve('target/debug/hey-boss'),binary);
 const env={...process.env,HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:join(root,'inbox.sock')};
 for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_STATE_DIR'])delete env[key];
 const project='named:HTML QA',body=readFileSync(new URL('../tests/fixtures/html-render.md',import.meta.url),'utf8');
 const cli=args=>JSON.parse(execFileSync(binary,args,{env,cwd:root,encoding:'utf8'}));
 let child,server,store,app,timer,inbox,closing=false;
 const abort=new AbortController();
 const json=async(url,options={})=>{
  const response=await fetch(url,{...options,signal:AbortSignal.any([abort.signal,AbortSignal.timeout(5000)])});
  const body=await response.json();
  if(!response.ok)throw Error(url+': '+JSON.stringify(body));
  return body;
 };
 const close=async()=>{
  if(closing)return;closing=true;clearInterval(timer);abort.abort();app?.locals.close();server?.closeAllConnections();
  if(server)await new Promise(resolve=>server.close(resolve));store?.close();
  if(inbox)await new Promise(resolve=>inbox.close(resolve));
  if(child&&child.exitCode===null){const exited=new Promise(resolve=>child.once('exit',resolve));child.kill('SIGTERM');const kill=setTimeout(()=>child.kill('SIGKILL'),2000);await exited;clearTimeout(kill);}
  rmSync(root,{recursive:true,force:true,maxRetries:10,retryDelay:50});
 };
 try{
  inbox=createSocket({allowHalfOpen:true},socket=>{
   socket.resume();socket.on('end',()=>socket.end(JSON.stringify({status:'ok',result:JSON.stringify({tasks:[],unread:0})})));
  });
  await new Promise(resolve=>inbox.listen(env.HEY_BOSS_INBOX_SOCKET,resolve));
  // Start the database owner before CLI writes, so no detached broker outlives
  // the fixture and recreates its files during cleanup.
  child=spawn(binary,['issue','web','--project',project,'--no-discovery','--port','0','--json'],{env,cwd:root,stdio:['ignore','pipe','inherit']});
  const lines=createInterface({input:child.stdout});
  const ready=await new Promise((resolve,reject)=>{lines.once('line',line=>resolve(JSON.parse(line)));child.once('exit',code=>reject(Error('Server exited '+code)));});
  cli(['project','init','--project','HTML QA','--prs','false','--worktree','false','--yes','--json']);
  const artifact=cli(['artifact','create','--project',project,'--title','HTML preview','--body',body,'--json']).artifact;
  const fullBody=readFileSync(new URL('../tests/fixtures/html-page.html',import.meta.url),'utf8');
  const full=cli(['artifact','create','--project',project,'--title','A complete HTML page','--body',fullBody,'--json']).artifact;
  const issue=cli(['issue','create','--project',project,'--title','HTML issue','--body',body,'--json']).issue;
  const native=ready.url.replace(/\/$/,''),boot=await json(native+'/api/bootstrap');
  store=new HubStore();store.setIssueProjects(boot.projects);
  server=createServer((req,res)=>app(req,res));await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const mobile='http://127.0.0.1:'+server.address().port,token='synthetic-html-fixture-bridge-token-12345';
  app=createApp({store,hubToken:token,origin:mobile,secure:false});
  app.get('/fixture-pairing',(req,res)=>res.json({code:store.pairing()}));
  let syncing=false;
  timer=setInterval(async()=>{
   if(syncing||closing)return;syncing=true;
   try{
    const current=await json(native+'/api/bootstrap');store.setIssueProjects(current.projects);
    const headers={'Content-Type':'application/json','X-Hey-Boss-CSRF':current.csrf};
    for(const request of store.pendingArtifacts()){
     const response=await json(native+'/api/action',{method:'POST',headers,body:JSON.stringify({project:request.project,operation:request.operation,request_id:request.id})});
     store.finishArtifact(request.id,response);
    }
    const requests=await json(mobile+'/api/bridge/web',{headers:{Authorization:'Bearer '+token}});
    for(const request of requests.requests){
     const response=await json(native+'/api/'+request.kind,{method:request.kind==='bootstrap'?'GET':'POST',headers,...(request.payload?{body:JSON.stringify(request.payload)}:{})});
     await json(mobile+'/api/bridge/web/'+request.id+'/result',{method:'POST',headers:{'Content-Type':'application/json',Authorization:'Bearer '+token},body:JSON.stringify(response)});
    }
   }catch(error){if(!closing)console.error(error.message);}finally{syncing=false;}
  },100);
  return {native,mobile,project,artifact:artifact.id,full:full.id,issue:issue.number,body,fullBody,close};
 }catch(error){await close();throw error;}
}
if(process.argv[1]===fileURLToPath(import.meta.url)){
 const fixture=await startFixture();console.log(JSON.stringify({...fixture,body:undefined,close:undefined}));
 for(const signal of ['SIGINT','SIGTERM'])process.on(signal,()=>fixture.close().then(()=>process.exit(0)));
}
