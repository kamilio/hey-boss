// Isolated native issue UI. Stop with Ctrl+C to remove the fixture.
import {mkdtempSync, rmSync, existsSync, realpathSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {createHash} from 'node:crypto';
import {createServer} from 'node:http';

const root = realpathSync(mkdtempSync(join(tmpdir(), 'hey-boss-scheduled-jobs-')));
const database = join(root, 'issues.db');
const env = {...process.env, HEY_BOSS_ISSUE_DB:database, HEY_BOSS_FLEET_STATE:root,
  HEY_BOSS_INBOX_SOCKET:join(root, 'inbox.sock'), HEY_BOSS_AGENT_ID:'human:boss'};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID']) delete env[key];
const binary = resolve(process.env.HEY_BOSS_TEST_BINARY || 'target/debug/hey-boss');
const children = [];
const start = args => {
  const child = spawn(binary, args, {cwd:root, env, stdio:['ignore','inherit','inherit']});
  children.push(child);
  return child;
};
const owner = start(['fleet','companion']);
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
  command(['project','init','--project','Quick action QA','--yes','--prs','false','--worktree','false','--json']);
  const records=[];
  for(const state of ['pending','running','failed','cancelled','succeeded']) {
    const id='visual-'+state, title={pending:'Review nightly build results',running:'Check dependency updates',failed:'Summarize repository activity',cancelled:'Inspect the release checklist',succeeded:'Review the weekly changelog'}[state];
    const job=['job','--project','Quick action QA','--agent','human:boss','--json'];
    command([...job,'create','--id',id,'--name',title,'--cron','0 9 * * *','--timezone','America/Chicago','--harness','codex','--model','exact-model','--instructions',resolve('src/jobs/fixtures/original.md'),'--paused']);
    const run=command([...job,'run-now',id]).run;
    const issue=['issue','--project','Quick action QA','--json'];
    if(state==='cancelled') command([...job,'stop',id,run.id]);
    else if(state!=='pending') {
      command([...issue,'status',String(run.task_number),state==='failed'?'red':state==='succeeded'?'green':'orange','--comment','Job '+state+(state==='failed'?': Chosen model is unavailable on this machine.':'')]);
      if(state==='succeeded')command([...issue,'close',String(run.task_number),'--force']);
      if(state==='failed')command([...issue,'unassign',String(run.task_number),'--force']);
    }
    records.push({state,number:run.task_number,id:run.id,title});
  }
  console.log(JSON.stringify({records}));
  start(['issue','--project','Quick action QA','web','--port','59639','--no-discovery','--json']);
  const native='http://127.0.0.1:59639';
  let boot;
  for (let attempt=0; !boot; attempt++) {
    if (attempt === 200) throw Error('Fixture web server did not start');
    try {boot=await (await fetch(native+'/api/bootstrap')).json();}
    catch {await new Promise(resolve=>setTimeout(resolve,50));}
  }
  server=createServer(async(req,res)=>{
    const send=(status,value)=>{res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(value));};
    try{
      if(req.method==='GET'&&req.url==='/fixture-runs.json')return send(200,records);
      send(404,{ok:false});
    }catch(error){send(500,{error:error.message});}
  });
  server.listen(52039,'127.0.0.1');
  console.log(JSON.stringify({fixture_pid:process.pid,owner_pid:owner.pid,native}));
} catch(error) {console.error(error);process.exitCode=1;await close();}
