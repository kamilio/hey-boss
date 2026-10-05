// Installed CLI regression in disposable state; optionally inspect with --serve.
import assert from 'node:assert/strict';
import {mkdtempSync, realpathSync, existsSync, readFileSync, writeFileSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {createHash} from 'node:crypto';
import {spawn, spawnSync} from 'node:child_process';
import {setTimeout as delay} from 'node:timers/promises';

const binary = resolve(process.argv[2]), expected = process.argv[3];
assert(expected, 'Pass binary and expected build ID');
const root = realpathSync(mkdtempSync(join(tmpdir(), 'hey-boss-reviewed-')));
const database = join(root, 'issues.db');
const socket = `/tmp/hey-boss-db-${process.getuid()}/${createHash('sha256').update(database).digest('hex').slice(0,24)}`;
const env = {...process.env, HEY_BOSS_ISSUE_DB:database, HEY_BOSS_FLEET_STATE:root,
  HEY_BOSS_INBOX_SOCKET:join(root,'inbox.sock'), HEY_BOSS_AGENT_ID:'human:boss'};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_STATE_DIR','CODEX_THREAD_ID']) delete env[key];
const children = [];
const start = args => {const child=spawn(binary,args,{env,cwd:root,stdio:['ignore','pipe','inherit']});children.push(child);return child;};
const run = (args, code=0) => {
  const r=spawnSync(binary,args,{env,cwd:root,encoding:'utf8',timeout:60000});
  assert.equal(r.status,code,r.stderr||r.stdout||String(r.error));
  return r.stdout;
};
const issue = (args, code=0) => JSON.parse(run(['issue','--project','Reviewed evidence QA','--json',...args],code));
let result;
try {
  const version=run(['--version']).trim();
  assert(version.includes(expected), 'Installed build mismatch');
  const owner=start(['fleet','companion']);
  for (let attempt=0;!existsSync(socket+'.sock');attempt++) {
    assert(attempt<600 && owner.exitCode===null && owner.signalCode===null, 'Fixture database failed to start');
    await delay(50);
  }
  const input=process.env.REVIEWED_EVIDENCE_FILE;
  const evidence=input?JSON.parse(readFileSync(input,'utf8')):Array.from({length:10},(_,index)=>({
    report:{data:{repository:'example/repo',number:index+1,
      pull_request:{number:index+1,state:'open',head:{sha:'head'},base:{ref:'main',sha:'base',repo:{full_name:'example/repo'}},user:{login:'author'}},
      conflicts:'clean',comments:[],review_comments:[],reviews:[{id:1,state:'COMMENTED',body:'Full reviewed diagnostic output.\n'.repeat(30000),user:{login:'reviewer'}}],
      timeline:[],review_events:[],review_threads:[],review_status:{requested_reviewers:[],requested_teams:[],latest_reviews:[],approved_by:[],changes_requested_by:[],dismissed_reviews:[],resolved_threads:0,unresolved_threads:0,outdated_threads:0},
      ci:{head_sha:'head',merge_sha:null,check_runs:[],commit_statuses:[],workflow_runs:[],jobs:[],summary:{state:'success',successful:0,failed:0,pending:0,skipped:0,unknown:0},failures:[],errors:[]},errors:[]},
      complete:true,observed_at_ms:100,oldest_validation_at_ms:100,validations:[]},
    policy:{repository:'example/repo',pull_number:index+1,head_sha:'head',base_branch:'main',base_sha:'base',pr_base_sha:'base',state:'not_required',strict:false,up_to_date:true,checks:[],rules:[],errors:[],cursor:'unused',pull_request_state:'open'}
  }));
  const raw=JSON.stringify(evidence), file=join(root,'reviewed.json');
  assert(Buffer.byteLength(raw)>8*1024*1024);
  assert.equal(evidence.length,10);
  issue(['create','--title','Ten reviewed pull requests','--body','Full evidence handoff verification.']);
  for(const {report} of evidence) issue(['pr','add','1',`https://github.com/${report.data.repository}/pull/${report.data.number}`]);
  issue(['claim','1']);issue(['ready','1']);
  const before=issue(['view','1']);
  const invalid=structuredClone(evidence);
  invalid.at(-1).policy.pr_base_sha='mismatched-base';
  writeFileSync(file,JSON.stringify(invalid));
  assert.equal(issue(['assign','1','github','--reviewed-evidence',file],4).error.code,'conflict');
  assert.deepEqual(issue(['view','1']),before,'Invalid last report must preserve the whole handoff');
  writeFileSync(file,raw);
  const args=['assign','1','github','--reviewed-evidence',file,'--if-version',String(before.issue.version),'--request-id','reviewed-once'];
  const assigned=issue(args);
  assert.equal(assigned.issue.state,'ready');
  assert.equal(assigned.issue.assignee,'watcher:github');
  assert.deepEqual(issue(args),assigned,'An identical retry must replay its receipt');
  const viewed=issue(['view','1']);
  assert.equal(viewed.issue.pull_requests.length,10);
  assert.equal(viewed.comment_count,before.comment_count);
  assert(run(['issue','assign','--help']).includes('64 MiB'));
  result={version,bytes:Buffer.byteLength(raw),reports:10,atomic_rejection:true,ready_handoff:true,idempotent_retry:true};
  if(process.argv.includes('--serve')) {
    const web=start(['issue','web','--port','0','--no-discovery','--project','Reviewed evidence QA','--json']);
    web.stdout.pipe(process.stdout);
    console.log(JSON.stringify({verified:result,fixture_pid:process.pid}));
    await new Promise(resolve=>{process.once('SIGTERM',resolve);process.once('SIGINT',resolve);});
  }
} finally {
  await Promise.all(children.map(child=>new Promise(resolve=>{
    if(child.exitCode!==null||child.signalCode!==null)return resolve();
    child.once('exit',resolve);child.kill('SIGTERM');
  })));
  rmSync(root,{recursive:true,force:true});
  for(const suffix of ['.sock','.lock','.startup','.log'])rmSync(socket+suffix,{force:true});
}
console.log(JSON.stringify({status:'passed',...result,cleanup_complete:true}));
