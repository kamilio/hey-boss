// Real installed scheduler and agent transport, with disposable state only.
import assert from 'node:assert/strict';
import {mkdtempSync, realpathSync, existsSync, writeFileSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {createHash} from 'node:crypto';
import {spawn, spawnSync} from 'node:child_process';
import {DatabaseSync} from 'node:sqlite';
import {setTimeout as delay} from 'node:timers/promises';

const binary = resolve(process.argv[2]), expectedBuild = process.argv[3];
const root = realpathSync(mkdtempSync(join(tmpdir(), 'hb-assignment-deadline-')));
const database = join(root, 'issues.db');
const socket = `/tmp/hey-boss-db-${process.getuid()}/${createHash('sha256').update(database).digest('hex').slice(0,24)}`;
writeFileSync(join(root,'inventory.json'), '{"ssh_hosts":[]}');
writeFileSync(join(root,'desired.json'), '{"machines":{}}');
const env = {...process.env, HEY_BOSS_ISSUE_DB:database, HEY_BOSS_FLEET_STATE:root,
  HEY_BOSS_FLEET_CONFIG:join(root,'inventory.json'), HEY_BOSS_FLEET_DESIRED:join(root,'desired.json'),
  HEY_BOSS_INBOX_SOCKET:join(root,'inbox.sock'),
  HEY_BOSS_CODEX:fileURLToPath(new URL('../tests/fixtures/agent-runtime.mjs',import.meta.url)),
  HEY_BOSS_FIXTURE_PROVIDER:'codex', HEY_BOSS_FIXTURE_JOB_HOLD:'1', HEY_BOSS_FIXTURE_FRESH_THREADS:'1'};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_STATE_DIR','HEY_BOSS_AGENT_ID','HEY_BOSS_WORKER_RUN','CODEX_THREAD_ID']) delete env[key];
const children = [];
let db;
const start = args => {
  const child = spawn(binary,args,{env,cwd:root,stdio:['ignore','ignore','pipe']});
  child.errors=''; child.stderr.on('data',chunk=>{child.errors=(child.errors+chunk).slice(-4000);});
  children.push(child); return child;
};
const run = args => {
  const result=spawnSync(binary,args,{env,cwd:root,encoding:'utf8',timeout:30000});
  assert.equal(result.status,0,result.stderr||result.stdout||String(result.error));
  return result.stdout;
};
const issue = (args, actor='human:verification') => JSON.parse(run(['issue','--project','Assignment deadline QA','--agent',actor,'--json',...args]));
const wait = async check => {
  const until=Date.now()+45000;
  while(!check()) {
    assert(Date.now()<until,'Timed out: '+children.map(c=>c.errors).join('\n'));
    await delay(100);
  }
};
async function stop(child) {
  if(child.exitCode!==null||child.signalCode!==null)return;
  const ended=new Promise(resolve=>child.once('exit',resolve));
  child.kill('SIGTERM');
  const timer=setTimeout(()=>child.kill('SIGKILL'),5000);
  await ended;clearTimeout(timer);
}
try {
  const version=run(['--version']).trim();
  assert(!expectedBuild||version.includes(expectedBuild),'Installed build mismatch');
  const service=start(['fleet','companion']);
  await wait(()=>existsSync(socket+'.sock'));
  run(['project','init','--project','Assignment deadline QA','--yes','--prs','false','--worktree','false','--json']);
  issue(['create','--title','Expired machine assignment verification','--body','']);
  db=new DatabaseSync(database);
  db.exec('PRAGMA busy_timeout=5000');
  const machine=db.prepare("SELECT json_extract(metadata,'$.machine') machine FROM agents WHERE id='human:verification'").get().machine;
  assert(machine);
  // Seed the peer's empty remote-history projection without connecting to a fleet.
  db.exec(`CREATE TABLE distributed_attempts(id TEXT PRIMARY KEY,machine TEXT NOT NULL,
    project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,state TEXT NOT NULL,
    started_at INTEGER NOT NULL,finished_at INTEGER,summary TEXT NOT NULL,payload TEXT NOT NULL);
    CREATE TABLE distributed_rejections(table_name TEXT NOT NULL,entity TEXT NOT NULL,
    reason TEXT NOT NULL,PRIMARY KEY(table_name,entity))`);
  db.prepare("UPDATE fleet_meta SET role='peer',node=?").run(machine);
  issue(['assign','1','machine:'+machine]);
  const original=issue(['view','1']).issue;
  assert.equal(original.assignment.machine,machine);
  const target=()=>db.prepare('SELECT assignment_target FROM issues WHERE number=1').get().assignment_target;
  assert.equal(target(),'machine:'+machine);
  db.exec('UPDATE fleet_allocation_deadlines SET expires_at=0');
  assert.equal(db.prepare('SELECT count(*) n FROM worker_runs').get().n,0);
  // Two actual worker processes race for the same expired assignment.
  const workerArgs=['worker','run','--project','Assignment deadline QA','--directory',root,'--no-prs','--no-worktree','--concurrency','1'];
  const first=start([...workerArgs,'--name','Deadline first']);
  const second=start([...workerArgs,'--name','Deadline second']);
  await wait(()=>db.prepare("SELECT count(*) n FROM worker_runs WHERE state='awaiting_claim' AND session_id IS NOT NULL").get().n===1);
  const attempt=db.prepare('SELECT * FROM worker_runs').get();
  assert.equal(db.prepare('SELECT count(*) n FROM worker_runs').get().n,1);
  assert.equal(db.prepare('SELECT count(*) n FROM fleet_allocation_deadlines').get().n,0);
  const queued=issue(['view','1']).issue;
  assert.equal(queued.assignment.machine,machine);
  assert.equal(queued.version,original.version);
  issue(['claim','1'],attempt.actor_id);
  const claimed=issue(['view','1']).issue;
  assert.equal(claimed.assignee,attempt.actor_id);
  assert.equal(claimed.agent_launch_count,1);
  // Expiry and stale observations must not steal an already launched/claimed run.
  db.exec(`INSERT OR REPLACE INTO fleet_allocation_deadlines SELECT project_id,issue_number,0 FROM fleet_allocations;
    UPDATE worker_allocation_observations SET deadline_tick=0;
    UPDATE worker_allocation_presence SET seen_tick=0;
    UPDATE worker_allocation_clock SET renewed_tick=0`);
  await delay(3500);
  const after=issue(['view','1']).issue;
  assert.equal(after.assignee,claimed.assignee);
  assert.equal(after.version,claimed.version);
  assert.equal(target(),'machine:'+machine);
  assert.equal(db.prepare('SELECT count(*) n FROM worker_runs').get().n,1);
  const active=db.prepare('SELECT * FROM worker_runs').get();
  assert.equal(active.id,attempt.id);
  assert.equal(active.finished_at,null);
  assert.equal(active.state,'running');
  assert.equal(db.prepare('SELECT id FROM worker_allocation_winners').get().id,attempt.id);
  for(const child of [service,first,second])assert.equal(child.exitCode,null,child.errors);
  console.log(JSON.stringify({status:'passed',version,attempts:1,agent_launches:1,owner_preserved:true,machine_preserved:true}));
} finally {
  for(const child of children.reverse())await stop(child);
  db?.close();
  rmSync(root,{recursive:true,force:true});
  for(const suffix of ['.sock','.lock','.startup','.log'])rmSync(socket+suffix,{force:true});
  console.log('Fixture processes and temporary files removed.');
}
