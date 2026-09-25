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
const root = realpathSync(mkdtempSync(join(tmpdir(), 'hey-boss-subtask-install-')));
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
const issue = (args, code=0) => JSON.parse(run(['issue','--project','Subtask installation QA','--json',...args],code));
let result;
try {
  const version=run(['--version']).trim();
  assert(version.includes(expectedBuild),'Installed build mismatch');
  const owner=start(['fleet','companion']);
  for (let attempt=0;!existsSync(socket+'.sock');attempt++) {
    assert(attempt<600 && owner.exitCode===null && owner.signalCode===null,'Fixture database failed to start');
    await delay(50);
  }
  for (const command of [[],['add'],['create'],['remove']]) {
    const help=run(['issue','subtask',...command,'--help']);
    for(const text of ['Blocked','claim','mindmap']) assert(help.includes(text),'Missing help: '+text);
  }
  issue(['create','--title','Parent']);
  issue(['create','--title','Follow-up']);
  issue(['claim','1']);
  issue(['claim','2']);
  const before=issue(['view','1']);
  for(const args of [['subtask','add','1','2'],['subtask','create','1','--title','Rejected child']]) {
    assert.equal(issue(args,4).error.code,'subtask_claim_conflict');
    assert.deepEqual(issue(['view','1']),before);
  }
  const mm = args => JSON.parse(run(['mm','--project','Subtask installation QA','--json',...args]));
  mm(['issue','1','--id','parent-work']);
  mm(['issue','2','--under','parent-work']);
  for(const number of ['1','2']) {
    const current=issue(['view',number]).issue;
    assert.equal(current.assignee,'codex:verification');
    assert.equal(current.state,'open');
    assert.equal(current.parent,null);
  }
  issue(['unassign','1']);
  issue(['subtask','add','1','2']);
  assert.equal(issue(['view','1']).issue.state,'blocked');
  assert.equal(issue(['view','2']).issue.assignee,'codex:verification');
  issue(['subtask','remove','1','2']);
  assert.equal(issue(['view','1']).issue.state,'open');
  assert.equal(issue(['create','--title','Next issue']).issue.number,3);
  const web=start(['issue','web','--port','0','--no-discovery','--project','Subtask installation QA','--json']);
  const info=await new Promise((yes,no)=>{
    let text='';const timer=setTimeout(()=>no(Error('Fixture web startup incomplete')),30000);
    web.once('error',no);web.once('exit',()=>{clearTimeout(timer);no(Error('Fixture web exited'));});
    web.stdout.on('data',chunk=>{text+=chunk;if(text.includes('\n')){clearTimeout(timer);try{yes(JSON.parse(text.split('\n')[0]));}catch(error){no(error);}}});
  });
  const html=await(await fetch(info.url)).text();
  const script=await(await fetch(new URL('/app.js',info.url))).text();
  assert(html.includes('id="editor-subtask-help"') && html.includes('id="subtask-picker-help"'));
  assert(script.includes('subtask_claim_conflict') && script.includes('$("#editor-subtask-help").hidden = !options.parent'));
  result={version,help_commands:4,claim_mutations_rejected:2,mindmap_claims_preserved:2,
    scheduling_verified:true,numbering_preserved:true,embedded_ui_verified:true};
} finally {
  await Promise.all(children.map(child=>new Promise(resolve=>{
    if(child.exitCode!==null||child.signalCode!==null)return resolve();
    child.once('exit',resolve);child.kill('SIGTERM');
  })));
  rmSync(root,{recursive:true,force:true});
  for(const suffix of ['.sock','.lock','.startup','.log'])rmSync(socket+suffix,{force:true});
}
console.log(JSON.stringify({status:'passed',...result,cleanup_complete:true}));
