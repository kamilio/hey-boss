// Run a fresh worker lifecycle through an installed binary in disposable state.
// --serve keeps its resulting issue UI available for visual inspection until SIGINT.
// --worker-binary PATH verifies a resident build against the installed service/CLI.
import assert from 'node:assert/strict';
import {mkdtempSync, realpathSync, existsSync, copyFileSync, chmodSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {createHash} from 'node:crypto';
import {spawn, spawnSync} from 'node:child_process';
import {setTimeout as delay} from 'node:timers/promises';

const binary = resolve(process.argv[2]), expectedBuild = process.argv[3];
const workerOption = process.argv.indexOf('--worker-binary');
assert(workerOption === -1 || process.argv[workerOption+1], '--worker-binary requires a path');
const workerBinary = workerOption === -1 ? binary : resolve(process.argv[workerOption+1]);
const root = realpathSync(mkdtempSync(join(tmpdir(), 'hb-requirements-handoff-')));
const database = join(root, 'issues.db');
const socket = `/tmp/hey-boss-db-${process.getuid()}/${createHash('sha256').update(database).digest('hex').slice(0,24)}`;
const env = {...process.env, HEY_BOSS_ISSUE_DB:database, HEY_BOSS_FLEET_STATE:root,
  HEY_BOSS_INBOX_SOCKET:join(root,'inbox.sock'), HEY_BOSS_CODEX:join(root,'codex-fixture.mjs'),
  HEY_BOSS_HANDOFF_TEST_BINARY:binary};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_STATE_DIR','HEY_BOSS_AGENT_ID','HEY_BOSS_WORKER_RUN','CODEX_THREAD_ID']) delete env[key];
const children = [];
const start = (args, executable = binary) => {
  const child = spawn(executable,args,{env,cwd:root,stdio:['ignore','pipe','pipe']});
  child.stderr.on('data', chunk => process.stderr.write(chunk));
  children.push(child);
  return child;
};
const run = (args, executable = binary) => {
  const result = spawnSync(executable,args,{env,cwd:root,encoding:'utf8',timeout:30000});
  assert.equal(result.status,0,result.stderr || result.stdout || String(result.error));
  return result.stdout;
};
const issue = args => JSON.parse(run(['issue','--project','Requirements handoff QA','--agent','human:verification','--json',...args]));
async function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const ended = new Promise(resolve => child.once('exit',resolve));
  child.kill('SIGTERM');
  const timer = setTimeout(() => child.kill('SIGKILL'),5000);
  await ended;
  clearTimeout(timer);
}
let interrupted, stopping = false;
const interrupt = new Promise(resolve => { interrupted = resolve; });
for (const signal of ['SIGINT','SIGTERM']) process.once(signal, () => {
  stopping = true;
  interrupted();
});
try {
  const version = run(['--version']).trim();
  const workerVersion = run(['--version'],workerBinary).trim();
  console.log(JSON.stringify({service_version:version,worker_version:workerVersion}));
  assert(!expectedBuild || version.includes(expectedBuild), 'Installed build mismatch');
  assert(run(['issue','ready','--help']).includes('--acknowledge-requirements'));
  copyFileSync(fileURLToPath(new URL('../tests/fixtures/requirements-handoff-worker.mjs',import.meta.url)),env.HEY_BOSS_CODEX);
  chmodSync(env.HEY_BOSS_CODEX,0o700);
  const owner = start(['fleet','companion']);
  for (let attempt=0; !existsSync(socket+'.sock'); attempt++) {
    assert(!stopping, 'Fixture interrupted');
    assert(attempt < 600 && owner.exitCode === null && owner.signalCode === null,'Fixture owner startup failed');
    await delay(50);
  }
  issue(['create','--title','Published delivery awaiting review','--body','Original requirements remain unchanged.']);
  issue(['pr','add','1','https://github.com/example/repo/pull/1','--purpose','fix']);
  const worker = start(['worker','run','--project','Requirements handoff QA','--directory',root,'--prs','--no-chief'],workerBinary);
  let status, finished;
  for (let attempt=0; !finished; attempt++) {
    await delay(200);
    assert(!stopping, 'Fixture interrupted');
    assert(worker.exitCode === null && worker.signalCode === null,
      `Worker exited before completion: ${workerVersion} (exit ${worker.exitCode}, signal ${worker.signalCode})`);
    status = issue(['worker','status']);
    finished = status.runs?.find(run => run.number === 1 && run.finished_at);
    assert(attempt < 300,'Worker lifecycle timed out: '+JSON.stringify(status));
  }
  assert.equal(finished.state,'completed',JSON.stringify({service_version:version,worker_version:workerVersion,
    state:finished.state,summary:finished.summary}));
  const view = issue(['view','1']);
  assert.equal(view.issue.state,'ready');
  assert.equal(view.issue.assignee,'watcher:github');
  assert.equal(view.issue.agent_launch_count,1);
  assert.equal(view.issue.closed_at,null);
  assert.equal(status.eligible,0);
  assert(view.issue.body.startsWith('Published handoff notes\n\n'));
  await delay(2500); // At least another pickup poll must leave the handoff parked.
  const after = issue(['worker','status']);
  assert.equal(after.runs.length,1,'Completion scheduled a redundant coding run');
  assert.equal(after.eligible,0);
  await stop(worker);
  console.log(JSON.stringify({status:'passed',version,worker_version:workerVersion,state:view.issue.state,assignee:view.issue.assignee,agent_launches:1,redundant_runs:0}));
  if (process.argv.includes('--serve')) {
    const web = start(['issue','web','--project','Requirements handoff QA','--port','0','--no-discovery','--json']);
    const info = await new Promise((resolve,reject) => {
      let text='';
      const timer=setTimeout(() => reject(Error('Web startup timed out')),30000);
      web.once('error',reject);
      web.stdout.on('data',chunk => {
        text+=chunk;
        if(text.includes('\n')) {clearTimeout(timer);resolve(JSON.parse(text.split('\n')[0]));}
      });
    });
    console.log(JSON.stringify({url:info.url,fixture_pid:process.pid}));
    // Keep unexpected service exits on the normal finally/cleanup path.
    // An unresolved top-level await alone exits Node without running finally.
    assert(web.exitCode === null && web.signalCode === null, 'Web fixture exited during startup');
    await Promise.race([interrupt, new Promise((_,reject) => {
      web.once('exit',(code,signal) => reject(Error(`Web fixture exited (code ${code}, signal ${signal})`)));
      owner.once('exit',(code,signal) => reject(Error(`Database fixture exited (code ${code}, signal ${signal})`)));
    })]);
  }
} finally {
  for (const child of children.reverse()) await stop(child);
  rmSync(root,{recursive:true,force:true});
  for (const suffix of ['.sock','.lock','.startup','.log']) rmSync(socket+suffix,{force:true});
  console.log('Fixture processes and temporary files removed.');
}
