// Real CLI processes and the deterministic agent fixture; no production writes.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {spawn, spawnSync} = require('node:child_process');
const {DatabaseSync} = require('node:sqlite');
const {createHash} = require('node:crypto');
const assert = require('node:assert/strict');
const withIssue=process.argv.includes('--with-issue');
const deadline=process.argv.includes('--deadline');

const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'hb-job-upgrade-')));
const source = path.resolve(process.env.HEY_BOSS_TEST_BINARY || 'target/debug/hey-boss');
const binary = path.join(root,'hey-boss');
fs.copyFileSync(source,binary);
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
  HEY_BOSS_FIXTURE_JOB_RELEASE:path.join(root,'release'),
  HEY_BOSS_TEST_CLI:binary,
};
if(withIssue){
  const runtime=env.HEY_BOSS_CODEX, issue=path.resolve('tests/fixtures/codex-worker.mjs');
  env.HEY_BOSS_CODEX=path.join(root,'codex.sh');
  fs.writeFileSync(env.HEY_BOSS_CODEX,`#!/bin/sh\nif [ -n "$HEY_BOSS_JOB_RUN" ]; then exec '${runtime}' "$@"; else exec '${issue}' "$@"; fi\n`,{mode:0o755});
  fs.writeFileSync(path.join(checkout,'mode.txt'),'delay-release');
}
for (const name of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID']) delete env[name];
const children = [];
let db;
const command = args => {
  const result = spawnSync(binary,args,{cwd:checkout,env,encoding:'utf8',timeout:15000});
  assert.equal(result.status,0,result.stdout+result.stderr);
  return JSON.parse(result.stdout);
};
const start = args => {
  const child = spawn(binary,args,{cwd:checkout,env,stdio:['ignore','pipe','pipe']});
  child.output='';child.stdout.on('data',b=>{child.output=(child.output+b).slice(-64000);});
  child.errors='';child.stderr.on('data',b=>{child.errors=(child.errors+b).slice(-4000);});
  children.push(child);return child;
};
const wait = async (check,timeout=20000) => {
  const until=Date.now()+timeout;
  while(!check()) { assert.ok(Date.now()<until,'Timed out: '+children.map(c=>c.errors).join('\n'));await new Promise(r=>setTimeout(r,100)); }
};
const stop = async child => {
  if(child.exitCode!==null||child.signalCode!==null)return;
  child.kill('SIGTERM');await wait(()=>child.exitCode!==null||child.signalCode!==null);
};
async function main(){
  let supervisor=start(['fleet','supervisor']);
  await wait(()=>fs.existsSync(path.join(state,'fleet.sock')));
  command(['project','init','--project','Worker fixture','--yes','--prs','false','--worktree','false','--json']);
  const worker=start(['worker','run','--name','Fixture worker','--concurrency','1','--no-worktree','--json']);
  db=new DatabaseSync(database);
  await wait(()=>db.prepare('SELECT count(*) n FROM local_job_workers').get().n===1);
  const workerId=db.prepare('SELECT worker_id FROM local_job_workers').get().worker_id;

  command(['job','create','--id','fixture-job','--name','Worker job','--cron','0 * * * *','--timezone','UTC','--harness','codex','--model','exact-model','--instructions',path.resolve('src/jobs/fixtures/original.md'),'--paused','--json']);
  const run=command(['job','run-now','fixture-job','--json']).run;
  await wait(()=>db.prepare('SELECT state FROM scheduled_job_runs WHERE id=?').get(run.id).state==='running');
  const execution=db.prepare('SELECT owner_pid,report FROM local_job_executions WHERE run_id=?').get(run.id);
  assert.equal(execution.owner_pid,worker.pid,'Worker process owns job execution');
  assert.equal(JSON.parse(execution.report).cwd,checkout,'Worker checkout policy applies');
  assert.equal(db.prepare('SELECT count(*) n FROM issues').get().n,0);
  assert.equal(db.prepare('SELECT count(*) n FROM worker_runs').get().n,0,'No task slot reserved');
  const status=command(['worker','status','--id',workerId,'--json']);
  assert.equal(status.active,0);assert.equal(status.free,1);
  assert.equal(status.workers.find(w=>w.id===workerId).job_active,1);
  if(withIssue){
    command(['issue','create','--title','Fixture issue','--json']);
    await wait(()=>db.prepare('SELECT count(*) n FROM worker_runs WHERE claimed_at IS NOT NULL AND finished_at IS NULL').get().n===1).catch(error=>{
      throw new Error(error.message+' '+JSON.stringify(db.prepare('SELECT state,summary,last_event FROM worker_runs').all()));
    });
  }
  const savedJob=db.prepare('SELECT * FROM scheduled_jobs WHERE id=?').get('fixture-job');
  const owner=db.prepare('SELECT * FROM scheduled_job_owners WHERE run_id=?').get(run.id);
  const before=JSON.parse(execution.report);
  const target=command(['worker','status','--id',workerId,'--json']).workers.find(w=>w.id===workerId).build;
  db.prepare("UPDATE issue_worker_builds SET build='previous-build' WHERE worker_id=?").run(workerId);
  fs.copyFileSync(source,binary+'.new');fs.renameSync(binary+'.new',binary);
  let rollout=start(['upgrade','--reload-workers',target,'--json']);
  if(deadline){
    await wait(()=>rollout.exitCode!==null,135000);
    assert.equal(rollout.exitCode,1,'Deadline must report incomplete');
    const report=JSON.parse(rollout.output);
    assert.equal(report.complete,false);
    assert.equal(report.workers.find(w=>w.id===workerId).state,'waiting_for_idle');
  }else await new Promise(r=>setTimeout(r,2500));
  const during=db.prepare('SELECT owner_pid,generation,report FROM local_job_executions WHERE run_id=?').get(run.id);
  assert.equal(JSON.parse(during.report).state,'running','Upgrade must not cancel an owned scheduled execution');
  assert.equal(during.owner_pid,worker.pid);
  assert.equal(during.generation,owner.generation);
  assert.equal(JSON.parse(during.report).session_id,before.session_id);
  assert.equal(db.prepare('SELECT stop_requested FROM issue_workers WHERE id=?').get(workerId).stop_requested,0);
  if(!deadline)assert.equal(rollout.exitCode,null,'Busy rollout must not report success');
  // Lost helper reply and supervisor handoff cannot stop or replay the occurrence.
  await stop(rollout);
  if(!deadline){
    await stop(supervisor);
    supervisor=start(['fleet','supervisor']);
    await wait(()=>fs.existsSync(path.join(state,'fleet.sock')));
  }
  rollout=start(['upgrade','--reload-workers',target,'--json']);
  await new Promise(r=>setTimeout(r,1000));
  assert.equal(JSON.parse(db.prepare('SELECT report FROM local_job_executions WHERE run_id=?').get(run.id).report).state,'running');
  fs.writeFileSync(env.HEY_BOSS_FIXTURE_JOB_RELEASE,'');
  if(withIssue){
    await wait(()=>db.prepare('SELECT state FROM scheduled_job_runs WHERE id=?').get(run.id).state==='succeeded');
    assert.equal(rollout.exitCode,null,'Remaining issue work also blocks reload');
    assert.equal(db.prepare('SELECT count(*) n FROM worker_runs WHERE claimed_at IS NOT NULL AND finished_at IS NULL').get().n,1);
    fs.writeFileSync(path.join(checkout,'release'),'');
  }
  await wait(()=>rollout.exitCode!==null).catch(error=>{
    throw new Error(error.message+' '+JSON.stringify({workerExit:worker.exitCode,workerErrors:worker.errors,workerOutput:worker.output.slice(-4000),status:command(['worker','status','--id',workerId,'--json'])}));
  });
  assert.equal(rollout.exitCode,0,rollout.errors);
  assert.equal(JSON.parse(rollout.output).complete,true);
  await wait(()=>db.prepare('SELECT state FROM scheduled_job_runs WHERE id=?').get(run.id).state==='succeeded');
  const after=command(['worker','status','--id',workerId,'--json']).workers.find(w=>w.id===workerId);
  assert.equal(after.pid,worker.pid,'Natural reload execs once in the original worker process');
  assert.equal(after.build,target);
  assert.deepEqual(after.config,status.workers.find(w=>w.id===workerId).config);
  assert.deepEqual(db.prepare('SELECT * FROM scheduled_jobs WHERE id=?').get('fixture-job'),savedJob,'Schedule and instruction revisions are untouched');
  assert.equal(db.prepare('SELECT generation FROM scheduled_job_owners WHERE run_id=?').get(run.id).generation,owner.generation);
  assert.equal(command(['upgrade','--reload-workers',target,'--json']).complete,true);
  assert.equal(fs.readFileSync(env.HEY_BOSS_FIXTURE_JOB_AUDIT,'utf8').trim().split('\n').length,1,'No replay after upgrade or lost acknowledgement');
  // Explicit user Stop remains immediate on a later occurrence.
  fs.unlinkSync(env.HEY_BOSS_FIXTURE_JOB_RELEASE);
  const second=command(['job','run-now','fixture-job','--json']).run;
  await wait(()=>db.prepare('SELECT state FROM scheduled_job_runs WHERE id=?').get(second.id).state==='running');
  command(['job','stop','fixture-job',second.id,'--json']);
  await wait(()=>db.prepare('SELECT state FROM scheduled_job_runs WHERE id=?').get(second.id).state==='cancelled');
  assert.equal(db.prepare('SELECT state FROM scheduled_job_runs WHERE id=?').get(run.id).state,'succeeded');
  assert.equal(db.prepare('SELECT count(*) n FROM issues').get().n,withIssue?1:0);
  console.log('Scheduled upgrade: original PID/session/generation, idle reload, lost reply, fleet recovery, no replay, unchanged schedule and immediate Stop passed.');
}
main().catch(e=>{console.error(e);process.exitCode=1;}).finally(async()=>{
  for(const child of children.reverse())await stop(child).catch(e=>{console.error(e);child.kill('SIGKILL');process.exitCode=1;});
  db?.close();fs.rmSync(root,{recursive:true,force:true});
  const identity=createHash('sha256').update(database).digest('hex').slice(0,24);
  for(const suffix of ['.sock','.lock','.startup','.log'])fs.rmSync(`/tmp/hey-boss-db-${process.getuid()}/${identity}${suffix}`,{force:true});
});
