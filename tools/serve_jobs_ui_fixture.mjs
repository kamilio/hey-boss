// Isolated native issue UI. Stop with Ctrl+C to remove the fixture.
import {mkdtempSync, rmSync, existsSync, realpathSync, writeFileSync, mkdirSync, readFileSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {DatabaseSync} from 'node:sqlite';
import {createHash} from 'node:crypto';
import {createServer} from 'node:http';

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
  const child = spawn(binary, args, {cwd:root, env, stdio:['ignore','inherit','inherit']});
  children.push(child);
  child.once('exit',()=>{if(!closing){process.exitCode=1;close();}});
  return child;
};
const owner = start(['fleet','supervisor']);
const identity = createHash('sha256').update(database).digest('hex').slice(0,24);
const socketBase = `/tmp/hey-boss-db-${process.getuid()}/${identity}`;
let server, closing = false;
async function close() {
  if (closing) return;
  closing = true;
  server?.closeAllConnections();
  server?.close();
  await Promise.all(children.map(child => new Promise(resolve => {
    if (child.exitCode !== null || child.signalCode !== null) return resolve();
    child.once('exit', resolve);
    child.kill('SIGTERM');
  })));
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
    const result=spawnSync(binary,args,{cwd:root,env,encoding:'utf8'});
    if(result.status!==0)throw Error(result.stdout+result.stderr);
    return JSON.parse(result.stdout);
  };
  command(['project','init','--project','Jobs UI QA','--yes','--prs','false','--worktree','false','--json']);
  start(['issue','--project','Jobs UI QA','web','--port','59649','--no-discovery','--json']);
  const native='http://127.0.0.1:59649';
  let boot;
  for (let attempt=0; !boot; attempt++) {
    if (attempt === 200) throw Error('Fixture web server did not start');
    try {boot=await (await fetch(native+'/api/bootstrap')).json();}
    catch {await new Promise(resolve=>setTimeout(resolve,50));}
  }
  server=createServer(async(req,res)=>{
    const send=(status,value)=>{res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(value));};
    try{
      if(req.method==='GET'&&req.url==='/fixture-instructions.md'){res.writeHead(200,{'Content-Type':'text/markdown'});return res.end(readFileSync('src/jobs/fixtures/original.md'));}
      let raw='';for await(const chunk of req)raw+=chunk;
      const body=raw?JSON.parse(raw):{};
      if(req.method==='POST'&&req.url==='/fixture-session'){
        const db=new DatabaseSync(database), session='aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee';
        try {
          const node=db.prepare('SELECT node FROM fleet_meta WHERE id=1').get().node;
          const updated=db.prepare("UPDATE scheduled_job_runs SET session_id=?,machine=? WHERE id=? AND state='cancelled'").run(session,node,body.run);
          if(!updated.changes)return send(400,{error:'Use a cancelled fixture run'});
          const path=join(root,'codex/sessions/2026/10/09');mkdirSync(path,{recursive:true});
          writeFileSync(join(path,'rollout-test-'+session+'.jsonl'),JSON.stringify({type:'response_item',payload:{type:'message',role:'assistant',content:[{type:'output_text',text:'Synthetic saved job result.'}]}})+'\n');
          return send(200,{ok:true,node});
        } finally {db.close();}
      }
      if(req.method==='POST'&&req.url==='/fixture-calendar-run'){
        const {run,state,at}=body;
        if(!['pending','running','succeeded','failed','cancelled','skipped'].includes(state)||!Number.isSafeInteger(at))return send(400,{ok:false});
        const db=new DatabaseSync(database);
        try {
          const reason={pending:'Chosen runtime is unavailable; waiting for a connected job service.',skipped:'overlap',failed:'Synthetic runtime failure',cancelled:'Stopped by user'}[state]||null;
          const updated=db.prepare("UPDATE scheduled_job_runs SET scheduled_at=?,created_at=?,started_at=?,finished_at=?,state=?,reason=? WHERE id=? AND project_id='named:Jobs UI QA' AND (state='skipped')=(?='skipped')").run(at,at+120000,['running','succeeded','failed'].includes(state)?at+180000:null,['succeeded','failed','cancelled','skipped'].includes(state)?at+240000:null,state,reason,run,state);
          return send(updated.changes?200:400,{ok:!!updated.changes});
        }finally{db.close();}
      }
      send(404,{ok:false});
    }catch(error){send(500,{error:error.message});}
  });
  server.listen(52049,'127.0.0.1');
  console.log(JSON.stringify({fixture_pid:process.pid,owner_pid:owner.pid,native}));
} catch(error) {console.error(error);process.exitCode=1;await close();}
