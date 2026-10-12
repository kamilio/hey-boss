// Real CLI processes and the deterministic agent fixture; no production writes.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {spawn, spawnSync} = require('node:child_process');
const {DatabaseSync} = require('node:sqlite');
const {createHash} = require('node:crypto');
const assert = require('node:assert/strict');

const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'hb-job-worker-')));
const binary = path.resolve(process.env.HEY_BOSS_TEST_BINARY || 'target/debug/hey-boss');
const checkout = path.join(root, 'checkout'), state = path.join(root, 'state');
for (const name of [checkout, state, path.join(root, 'home')]) fs.mkdirSync(name);
fs.writeFileSync(path.join(root, 'inventory.json'), '{"ssh_hosts":[]}');
fs.writeFileSync(path.join(root, 'desired.json'), '{"machines":{}}');
const database = path.join(root, 'issues.db');
const env = {...process.env, HOME:path.join(root,'home'), CODEX_HOME:path.join(root,'codex'),
  HEY_BOSS_ISSUE_DB:database, HEY_BOSS_FLEET_STATE:state,
  HEY_BOSS_FLEET_CONFIG:path.join(root,'inventory.json'), HEY_BOSS_FLEET_DESIRED:path.join(root,'desired.json'),
  HEY_BOSS_INBOX_SOCKET:path.join(root,'inbox.sock'), HEY_BOSS_AGENT_ID:'human:fixture',
  HEY_BOSS_CODEX:path.resolve('tests/fixtures/agent-runtime.mjs'), HEY_BOSS_FIXTURE_PROVIDER:'codex',
  HEY_BOSS_FIXTURE_EXPECT_MODEL:'exact-model', HEY_BOSS_FIXTURE_JOB_HOLD:'1',
  HEY_BOSS_FIXTURE_JOB_AUDIT:path.join(root,'launches.jsonl'),
};
for (const name of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID']) delete env[name];
const children = [];
let db;
const command = args => {
  const result = spawnSync(binary,args,{cwd:checkout,env,encoding:'utf8',timeout:15000});
  assert.equal(result.status,0,result.stdout+result.stderr);
  return JSON.parse(result.stdout);
};
const start = args => {
  const child = spawn(binary,args,{cwd:checkout,env,stdio:['ignore','ignore','pipe']});
  child.errors='';child.stderr.on('data',b=>{child.errors=(child.errors+b).slice(-4000);});
  children.push(child);return child;
};
const wait = async check => {
  const until=Date.now()+20000;
  while(!check()) { assert.ok(Date.now()<until,'Timed out: '+children.map(c=>c.errors).join('\n'));await new Promise(r=>setTimeout(r,100)); }
};
const stop = async child => {
  if(child.exitCode!==null||child.signalCode!==null)return;
  child.kill('SIGTERM');await wait(()=>child.exitCode!==null||child.signalCode!==null);
};
async function main(){
  let supervisor=start(['fleet','supervisor']);
  await wait(()=>fs.existsSync(path.join(state,'fleet.sock')));
  command(['project','init','--project','Job worker QA','--yes','--prs','false','--worktree','true','--json']);
  const worker=start(['worker','run','--name','Fixture worker','--concurrency','1','--worktree','--json']);
  db=new DatabaseSync(database,{readOnly:true});
  await wait(()=>db.prepare('SELECT count(*) n FROM local_job_workers').get().n===1);
  const workerId=db.prepare('SELECT worker_id FROM local_job_workers').get().worker_id;
  // Register as a local project first so the fixture needs no GitHub credentials.
  for(const args of [['init','-q','--initial-branch=legacy'],['-c','user.name=Fixture','-c','user.email=fixture@example.invalid','-c','core.hooksPath=/dev/null','commit','--allow-empty','-qm','fixture']]) {
    const r=spawnSync('git',args,{cwd:checkout,env,encoding:'utf8'});assert.equal(r.status,0,r.stderr);
  }

  command(['job','create','--id','fixture-job','--name','Worker job','--cron','0 * * * *','--timezone','UTC','--harness','codex','--model','exact-model','--instructions',path.resolve('src/jobs/fixtures/original.md'),'--paused','--json']);
  const run=command(['job','run-now','fixture-job','--json']).run;
  await wait(()=>db.prepare('SELECT state FROM scheduled_job_runs WHERE id=?').get(run.id).state==='running');
  const execution=db.prepare('SELECT owner_pid,report FROM local_job_executions WHERE run_id=?').get(run.id);
  assert.equal(execution.owner_pid,worker.pid,'Worker process owns job execution');
  assert.notEqual(JSON.parse(execution.report).cwd,checkout,'Worker worktree policy applies');
  assert.equal(db.prepare('SELECT count(*) n FROM issues').get().n,0);
  assert.equal(db.prepare('SELECT count(*) n FROM worker_runs').get().n,0,'No task slot reserved');
  const status=command(['worker','status','--id',workerId,'--json']);
  assert.equal(status.active,0);assert.equal(status.free,1);
  assert.equal(status.workers.find(w=>w.id===workerId).job_active,1);
  await stop(supervisor);
  assert.equal(worker.exitCode,null,'Fleet shutdown leaves the worker alive');
  assert.equal(JSON.parse(db.prepare('SELECT report FROM local_job_executions WHERE run_id=?').get(run.id).report).state,'running');
  supervisor=start(['fleet','supervisor']);
  await wait(()=>fs.existsSync(path.join(state,'fleet.sock')));
  command(['job','stop','fixture-job',run.id,'--json']);
  await wait(()=>db.prepare('SELECT state FROM scheduled_job_runs WHERE id=?').get(run.id).state==='cancelled');
  assert.equal(fs.readFileSync(env.HEY_BOSS_FIXTURE_JOB_AUDIT,'utf8').trim().split('\n').length,1,'No replay after fleet restart');
  assert.equal(db.prepare('SELECT count(*) n FROM issues').get().n,0);
  console.log('Worker-owned Jobs: real CLI ownership, worktree, slot accounting, fleet restart, exact stop and no task creation passed.');
}
main().catch(e=>{console.error(e);process.exitCode=1;}).finally(async()=>{
  for(const child of children.reverse())await stop(child).catch(e=>{console.error(e);child.kill('SIGKILL');process.exitCode=1;});
  db?.close();fs.rmSync(root,{recursive:true,force:true});
  const identity=createHash('sha256').update(database).digest('hex').slice(0,24);
  for(const suffix of ['.sock','.lock','.startup','.log'])fs.rmSync(`/tmp/hey-boss-db-${process.getuid()}/${identity}${suffix}`,{force:true});
});
