// Isolated native and paired issue UI. Stop with Ctrl+C to remove the fixture.
import {mkdtempSync, rmSync, existsSync, realpathSync, writeFileSync, mkdirSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {DatabaseSync} from 'node:sqlite';
import {createHash} from 'node:crypto';
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';

const root = realpathSync(mkdtempSync(join(tmpdir(), 'hey-boss-jobs-ui-')));
const database = join(root, 'issues.db');
writeFileSync(join(root,'inventory.json'),'{"ssh_hosts":[]}');
writeFileSync(join(root,'desired.json'),'{"machines":{}}');
const env = {...process.env, CODEX_HOME:join(root,'codex'), HEY_BOSS_FLEET_CONFIG:join(root,'inventory.json'), HEY_BOSS_FLEET_DESIRED:join(root,'desired.json'), HEY_BOSS_ISSUE_DB:database, HEY_BOSS_FLEET_STATE:root,
  HEY_BOSS_INBOX_SOCKET:join(root, 'inbox.sock'), HEY_BOSS_AGENT_ID:'human:boss'};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID']) delete env[key];
const binary = resolve(process.env.HEY_BOSS_TEST_BINARY || 'target/debug/hey-boss');
const children = [];
const start = args => {
  const child = spawn(binary, args, {env, stdio:['ignore','inherit','inherit']});
  children.push(child);
  child.once('exit',()=>{if(!closing){process.exitCode=1;close();}});
  return child;
};
const owner = start(['fleet','supervisor']);
const identity = createHash('sha256').update(database).digest('hex').slice(0,24);
const socketBase = `/tmp/hey-boss-db-${process.getuid()}/${identity}`;
let timer, app, server, store, closing = false;
async function close() {
  if (closing) return;
  closing = true;
  clearTimeout(timer);
  app?.locals.close();
  server?.closeAllConnections();
  server?.close();
  await Promise.all(children.map(child => new Promise(resolve => {
    if (child.exitCode !== null || child.signalCode !== null) return resolve();
    child.once('exit', resolve);
    child.kill('SIGTERM');
  })));
  store?.close();
  rmSync(root, {recursive:true,force:true});
  for (const suffix of ['.sock','.lock','.startup','.log']) rmSync(socketBase+suffix,{force:true});
  console.log('COMPLETE: fixture processes stopped and temporary data removed');
}
for (const signal of ['SIGTERM','SIGINT']) process.on(signal, close);
try {
  for (let attempt=0; !existsSync(socketBase+'.sock'); attempt++) {
    if (attempt === 200 || owner.exitCode !== null) throw Error('Fixture database did not start');
    await new Promise(resolve=>setTimeout(resolve,50));
  }
  const command = args => {
    const result=spawnSync(binary,args,{env,encoding:'utf8'});
    if(result.status!==0)throw Error(result.stdout+result.stderr);
    return JSON.parse(result.stdout);
  };
  command(['project','init','--project','Jobs UI QA','--yes','--prs','false','--worktree','false','--json']);
  start(['issue','--project','Jobs UI QA','web','--port','59649','--no-discovery','--json']);
  const native='http://127.0.0.1:59649', paired='http://127.0.0.1:52049';
  let boot;
  for (let attempt=0; !boot; attempt++) {
    if (attempt === 200) throw Error('Fixture web server did not start');
    try {boot=await (await fetch(native+'/api/bootstrap')).json();}
    catch {await new Promise(resolve=>setTimeout(resolve,50));}
  }
  store=new HubStore();store.setIssueProjects(boot.projects);
  const headers={'Content-Type':'application/json',Authorization:'Bearer '+'synthetic-quick-actions-fixture'.repeat(2)};
  app=createApp({store,hubToken:'synthetic-quick-actions-fixture'.repeat(2),origin:paired,secure:false});
  app.post('/fixture-session',(req,res)=>{
    const db=new DatabaseSync(database), session='aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee';
    try {
      const node=db.prepare('SELECT node FROM fleet_meta WHERE id=1').get().node;
      const updated=db.prepare("UPDATE scheduled_job_runs SET session_id=?,machine=? WHERE id=? AND state='cancelled'").run(session,node,req.body.run);
      if(!updated.changes)return res.status(400).json({error:'Use a cancelled fixture run'});
      const path=join(root,'codex/sessions/2026/10/09');mkdirSync(path,{recursive:true});
      writeFileSync(join(path,'rollout-test-'+session+'.jsonl'),JSON.stringify({type:'response_item',payload:{type:'message',role:'assistant',content:[{type:'output_text',text:'Synthetic saved job result.'}]}})+'\n');
      res.json({ok:true,node});
    } finally {db.close();}
  });
  app.get('/fixture-instructions.md',(_req,res)=>res.sendFile(resolve('src/jobs/fixtures/original.md')));
  app.get('/fixture-pairing',(_req,res)=>res.json({code:store.pairing()}));
  server=app.listen(52049,'127.0.0.1');
  async function relay() {
    try {
      const snapshot=await(await fetch(native+'/api/fleet/status')).json();
      await fetch(paired+'/api/bridge/agents/status',{method:'POST',headers,body:JSON.stringify(snapshot)});
      const agents=await(await fetch(paired+'/api/bridge/agents',{headers})).json();
      for(const request of agents.requests){
        const query=new URLSearchParams({host:request.host,run:request.run,project:request.project,cursor:request.cursor||0});
        const result=await(await fetch(native+'/api/fleet/conversation?'+query)).json();
        await fetch(paired+'/api/bridge/agents/'+request.id+'/result',{method:'POST',headers,body:JSON.stringify(result)});
      }
      const queue=await (await fetch(paired+'/api/bridge/web',{headers})).json();
      for (const request of queue.requests) {
        const response=request.kind==='inbox'?{ok:true,tasks:[]}:
          await (await fetch(native+'/api/'+request.kind,request.kind==='bootstrap'?{}:{method:'POST',
            headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf},body:JSON.stringify(request.payload)})).json();
        await fetch(paired+'/api/bridge/web/'+request.id+'/result',{method:'POST',headers,body:JSON.stringify(response)});
      }
    } catch(error) {if(!closing) console.error(error);}
    if(!closing) timer=setTimeout(relay,50);
  }
  relay();
  console.log(JSON.stringify({fixture_pid:process.pid,owner_pid:owner.pid,native,paired:paired+'/issues'}));
} catch(error) {console.error(error);process.exitCode=1;await close();}
