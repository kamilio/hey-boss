// Verify an installed build in disposable state, including its embedded web UI.
import assert from 'node:assert/strict';
import {mkdtempSync, realpathSync, existsSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {createHash} from 'node:crypto';
import {spawn, spawnSync} from 'node:child_process';
import {setTimeout as delay} from 'node:timers/promises';

const binary = resolve(process.argv[2]), expectedBuild = process.argv[3];
assert(expectedBuild, 'Pass the expected installed build ID');
const root = realpathSync(mkdtempSync(join(tmpdir(), 'hey-boss-dependencies-install-')));
const database = join(root, 'issues.db');
const socket = `/tmp/hey-boss-db-${process.getuid()}/${createHash('sha256').update(database).digest('hex').slice(0,24)}`;
const env = {...process.env, HEY_BOSS_ISSUE_DB:database, HEY_BOSS_FLEET_STATE:root,
  HEY_BOSS_INBOX_SOCKET:join(root,'inbox.sock'), HEY_BOSS_AGENT_ID:'codex:verification'};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_STATE_DIR','CODEX_THREAD_ID']) delete env[key];
const children = [];
const start = args => {const child=spawn(binary,args,{env,cwd:root,stdio:['ignore','pipe','inherit']});children.push(child);return child;};
const run = (args, code=0) => {
  const r=spawnSync(binary,args,{env,cwd:root,encoding:'utf8',timeout:30000});
  assert.equal(r.signal,null,'Command interrupted: '+args.join(' '));
  assert.equal(r.status,code,r.stderr||r.stdout||String(r.error));
  return r.stdout;
};
const issue = (args, code=0) => JSON.parse(run(['issue','--project','Dependency installation QA','--json',...args],code));
const sql = (statement,args=[]) => {
  const r=spawnSync(binary,['fleet','database','--path',database],{env,cwd:root,encoding:'utf8',timeout:30000,input:JSON.stringify({sql:statement,args})+'\n'});
  assert.equal(r.signal,null,'Database probe interrupted');
  assert.equal(r.status,0,r.stderr||r.stdout||String(r.error));
  const response=JSON.parse(r.stdout);assert.equal(response.ok,true,JSON.stringify(response));return response.rows;
};
let result;
try {
  const version=run(['--version']).trim();
  assert(version.includes(expectedBuild),'Installed build mismatch');
  const owner=start(['fleet','companion']);
  for (let attempt=0;!existsSync(socket+'.sock');attempt++) {
    assert(attempt<600 && owner.exitCode===null && owner.signalCode===null,'Fixture database failed to start');
    await delay(50);
  }
  assert(run(['issue','settings','set','--help']).includes('--subtask-scheduling'));
  assert(run(['issue','reopen','--help']).includes('--clear-manual-hold'));
  issue(['create','--title','Parent']);
  for(const title of ['Contract','Independent Settings','OAuth']) issue(['subtask','create','1','--title',title]);
  assert.equal(issue(['view','3']).issue.state,'blocked');
  issue(['settings','set','--subtask-scheduling','explicit']);
  assert.equal(issue(['settings','show']).subtask_scheduling,'explicit');
  assert.equal(issue(['view','3']).issue.state,'open');
  assert.equal(issue(['view','3']).issue.parent.number,1);
  assert.deepEqual(issue(['view','3']).issue.blocked_by,[]);
  issue(['claim','3']);
  const independent=issue(['view','3']).issue;
  const legacyBody='Dependency rework: upstream tasks [2] need work. Read their latest changes and update/rebase the stacked PR before marking this task Ready. Running worker claims are preserved; new pickups wait for the dependencies.';
  sql("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('named:Dependency installation QA',3,'codex:verification',?,123)",[legacyBody]);
  sql("INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES('named:Dependency installation QA',3,'codex:verification','commented',123,?)",[JSON.stringify({comment_id:1,body:legacyBody})]);
  sql("INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES('named:Dependency installation QA',3,'codex:verification','dependency_rework',123,?)",[JSON.stringify({dependencies:[[2,1]]})]);
  assert.equal(sql('SELECT count(*) FROM comments WHERE created_at=123')[0][0],0);
  assert.equal(sql('SELECT count(*) FROM events WHERE created_at=123')[0][0],0);
  assert.deepEqual(issue(['view','3']).issue,independent);
  issue(['blocked-by','4','2']);
  issue(['block','4']);
  const before=issue(['view','4']).issue;
  const rejected=issue(['reopen','4'],4);
  assert(rejected.error.message.includes('#2 (linked)'));
  assert.equal(rejected.error.details.blocked_by[0].source,'linked');
  issue(['reopen','4','--clear-manual-hold','--if-version',String(before.version)]);
  const held=issue(['view','4']).issue;
  assert.equal(held.manual_blocked,false);
  assert.equal(held.state,'blocked');
  assert.deepEqual(held.blocker_numbers,[2]);
  issue(['settings','set','--prs-enabled']);
  issue(['pr','add','2','https://github.com/example/connectors/pull/2']);
  issue(['ready','2']);
  assert.equal(issue(['view','4']).issue.state,'open');
  issue(['claim','4']);
  issue(['settings','set','--subtask-scheduling','sequential'],4);
  assert.equal(issue(['settings','show']).subtask_scheduling,'explicit');
  assert.equal(issue(['view','4']).issue.assignee,'codex:verification');
  assert.equal(issue(['subtask','create','4','--title','Unsafe child'],4).error.code,'subtask_claim_conflict');
  assert.equal(issue(['view','4']).issue.assignee,'codex:verification');
  issue(['reopen','2']);
  const rework=issue(['view','4']);
  assert.equal(rework.issue.assignee,'codex:verification');
  assert(rework.comments.some(c=>c.body.startsWith('Dependency rework: upstream tasks [2]')));
  assert.equal(issue(['view','3']).comments.length,0);
  const web=start(['issue','web','--port','0','--no-discovery','--project','Dependency installation QA','--json']);
  const info=await new Promise((yes,no)=>{
    let text='';const timer=setTimeout(()=>no(Error('Fixture web startup incomplete')),30000);
    web.once('error',no);web.once('exit',()=>{clearTimeout(timer);no(Error('Fixture web exited'));});
    web.stdout.on('data',chunk=>{text+=chunk;if(text.includes('\n')){clearTimeout(timer);try{yes(JSON.parse(text.split('\n')[0]));}catch(error){no(error);}}});
  });
  const html=await(await fetch(info.url)).text();
  const script=await(await fetch(new URL('/app.js',info.url))).text();
  const settings=await(await fetch(new URL('/project-settings.js',info.url))).text();
  assert(html.includes('id="project-subtask-scheduling"'));
  assert(script.includes('clear_manual_hold') && script.includes('Waiting for dependencies'));
  assert(settings.includes('subtask_scheduling'));
  result={version,completed:6,expected:6,stages:['CLI discovery','independent siblings and grouping','legacy notice admission and unchanged claims','manual hold and structured blockers','Ready handoff and real rework','embedded UI']};
} finally {
  await Promise.all(children.map(child=>new Promise(resolve=>{
    if(child.exitCode!==null||child.signalCode!==null)return resolve();
    child.once('exit',resolve);child.kill('SIGTERM');
  })));
  rmSync(root,{recursive:true,force:true});
  for(const suffix of ['.sock','.lock','.startup','.log'])rmSync(socket+suffix,{force:true});
}
console.log(JSON.stringify({status:'passed',...result,cleanup_complete:true}));
