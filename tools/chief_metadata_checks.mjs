// Qualify installed Chief metadata routing through a real private fleet connection.
import assert from 'node:assert/strict';
import {mkdtempSync, mkdirSync, writeFileSync, chmodSync, copyFileSync, rmSync, existsSync, realpathSync} from 'node:fs';
import {spawn, spawnSync} from 'node:child_process';
import {join, resolve} from 'node:path';
import {createHash} from 'node:crypto';

const root=realpathSync(mkdtempSync('/tmp/hb-authority-'));
// Keep this run's executable stable while other checks rebuild target/debug.
// The viewer deliberately reloads when its executable is replaced.
const binary=join(root,'hey-boss');
const main=join(root,'main'), peer=join(root,'peer'), bin=join(root,'bin');
const children=[], checks=[];
const serve=process.argv.includes('--serve');
const check=name=>{checks.push(name);console.log('PASS: '+name);};
let closing=false;
let supervisor;
for(const path of [main,peer,bin]) mkdirSync(path);
for(const path of [main,peer]) {
  writeFileSync(join(path,'inventory.json'), JSON.stringify({ssh_hosts:path===main?['fixture.test']:[]}));
  writeFileSync(join(path,'desired.json'), '{"machines":{}}');
}
const envFor=path=>{
  const env={...process.env,HEY_BOSS_ISSUE_DB:join(path,'issues.db'),HEY_BOSS_FLEET_STATE:path,
    HEY_BOSS_FLEET_CONFIG:join(path,'inventory.json'),HEY_BOSS_FLEET_DESIRED:join(path,'desired.json'),
    HEY_BOSS_INBOX_SOCKET:join(path,'absent.sock'),HEY_BOSS_TEST_CLI:binary,AUTHORITY_PEER:peer};
  for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID']) delete env[key];
  return env;
};
const start=(path,args,extra={})=>{
  const child=spawn(binary,args,{cwd:path,env:{...envFor(path),...extra},stdio:['ignore','pipe','inherit']});
  children.push(child); return child;
};
const cli=(path,args,expected=0)=>new Promise((resolve,reject)=>{
  const child=start(path,args); let output='';
  child.stdout.on('data',chunk=>output+=chunk);
  child.once('error',reject);
  const timer=setTimeout(()=>{reject(Error('CLI timed out: '+args.join(' ')));child.kill('SIGTERM');},30000);
  child.once('exit',(code,signal)=>{clearTimeout(timer);code===expected&&!signal?resolve(JSON.parse(output)):reject(Error(`CLI incomplete (${path}: ${args.join(' ')}): code=${code}, signal=${signal}: ${output}`));});
});
const wait=ms=>new Promise(resolve=>setTimeout(resolve,ms));
function startSupervisor(){
  supervisor=start(main,['fleet','supervisor'],{PATH:`${bin}:${process.env.PATH}`});
  supervisor.stdout.resume();
}
async function close(){
  if(closing)return;closing=true;
  for(const child of [...children].reverse()) {
    if(child.exitCode!==null||child.signalCode!==null)continue;
    const result=await new Promise(resolve=>{child.once('exit',(code,signal)=>resolve({code,signal}));child.kill('SIGTERM');});
    assert.equal(result.code,0,'Fixture service did not exit normally');
    assert.equal(result.signal,null,'Fixture service was interrupted');
  }
  // The supervisor may terminate its stdio child before Rust destructors run.
  // Remove only this fixture's socket, after verifying no process still owns it.
  for(let attempt=0;existsSync(join(peer,'fleet-authority.sock'));attempt++) {
    const owners=spawnSync('lsof',['-t',join(peer,'fleet-authority.sock')],{encoding:'utf8'});
    if(owners.status===1&&!owners.stdout.trim())break;
    if(owners.error||owners.signal||attempt===100)throw Error('Fixture relay ownership could not be cleared');
    await wait(50);
  }
  for(const path of [main,peer]) {
    const id=createHash('sha256').update(join(path,'issues.db')).digest('hex').slice(0,24);
    for(const suffix of ['.sock','.lock','.startup','.log']) rmSync(`/tmp/hey-boss-db-${process.getuid()}/${id}${suffix}`,{force:true});
  }
  rmSync(root,{recursive:true,force:true});
  console.log('COMPLETE: Chief metadata fixture stopped; temporary files removed');
}
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,close);
// A resumed tool session can lose its output pipe. Still clean up the fixture
// instead of crashing and leaving its supervisor and database owners behind.
process.stdout.on('error',()=>{process.exitCode=1;void close();});
try {
  copyFileSync(resolve(process.env.HEY_BOSS_TEST_BINARY || 'target/debug/hey-boss'),binary);
  chmodSync(binary,0o700);
  // These owned services host SQLite for the fixture and shut it down with us.
  for(const path of [main,peer]) {
    const owner=start(path,['fleet','companion']);owner.stdout.resume();
    console.log(JSON.stringify({starting:'database',pid:owner.pid,root:path}));
    const id=createHash('sha256').update(join(path,'issues.db')).digest('hex').slice(0,24);
    for(let attempt=0;!existsSync(`/tmp/hey-boss-db-${process.getuid()}/${id}.sock`);attempt++) {
      if(attempt===1200||owner.exitCode!==null||owner.signalCode!==null)throw Error(`Fixture database failed to start: code=${owner.exitCode}, signal=${owner.signalCode}`);
      await wait(50);
    }
  }
  const issue=(path,args,code=0)=>cli(path,['issue','--project','Chief metadata QA','--agent','codex:chief-fixture','--json',...args],code);
  const database=(path,request)=>{
    const result=spawnSync(binary,['fleet','database','--path',join(path,'issues.db')],{env:envFor(path),cwd:path,input:JSON.stringify(request)+'\n',encoding:'utf8',timeout:30000});
    assert.equal(result.signal,null);assert.equal(result.status,0,result.stderr);
    const data=JSON.parse(result.stdout);assert.equal(data.ok,true,JSON.stringify(data));return data.rows;
  };
  const sql=async(path,statement,args=[])=>database(path,{sql:statement,args});
  writeFileSync(join(bin,'ssh'),'#!/bin/sh\nexport HEY_BOSS_ISSUE_DB="$AUTHORITY_PEER/issues.db" HEY_BOSS_FLEET_STATE="$AUTHORITY_PEER" HEY_BOSS_FLEET_CONFIG="$AUTHORITY_PEER/inventory.json" HEY_BOSS_FLEET_DESIRED="$AUTHORITY_PEER/desired.json"\n"$HEY_BOSS_TEST_CLI" fleet companion --stdio\nresult=$?; exit "$result"\n');
  chmodSync(join(bin,'ssh'),0o700);
  startSupervisor();
  for(let attempt=0;!existsSync(join(main,'fleet.sock'));attempt++) {
    assert(attempt<600,'Supervisor startup incomplete');await wait(100);
  }
  // Two fixture processes share one physical UUID. Give authority history a
  // distinct origin before seeding, as it has on a real second device.
  database(main,{replica:'capture',role:'controller',node:'main'});
  await issue(main,['create','--title','Delivered fix — label cleanup','--label','rework needed','--body','## Verified delivery\n\nThe fix is delivered. Remove the stale triage label without reopening this issue.']);
  await issue(main,['close','1']);
  await issue(main,['create','--title','Active worker — metadata review','--body','## Worker in progress\n\nReview labels while preserving the running assignment and its reservation.']);
  await cli(main,['issue','--project','Chief metadata QA','--agent','codex:active-worker','--json','claim','2']);
  await sql(main,"INSERT INTO fleet_allocations VALUES('named:Chief metadata QA',2,'active-worker-machine') ON CONFLICT(project_id,issue_number) DO UPDATE SET node=excluded.node");
  await sql(main,"INSERT INTO worker_runs(id,project_id,issue_number,actor_id,state,started_at,updated_at,reservation_expires,claimed_at,job,owner_pid,owner_start,machine) VALUES('fixture-live','named:Chief metadata QA',2,'codex:active-worker','running',1,1,9223372036854775807,1,'{}',123,'fixture','active-worker-machine')");
  const ownership=async()=>({issues:await sql(main,'SELECT number,state,assignee FROM issues ORDER BY number'),allocations:await sql(main,'SELECT * FROM fleet_allocations'),runs:await sql(main,'SELECT * FROM worker_runs')});
  const before=await ownership();
  for(let attempt=0;;attempt++) {
    try {
      assert(existsSync(join(peer,'fleet-authority.sock')),'Waiting for the companion relay');
      const ready=await issue(peer,['--supervisor','view','1']);
      assert.equal(ready.issue.title,'Delivered fix — label cleanup');
      break;
    } catch(error){if(attempt===600)throw error;await wait(100);}
  }
  check('Authenticated supervisor connection established');
  for(const number of ['1','2']) {
    const initial=await issue(peer,['--supervisor','view',number]);
    assert.equal(initial.store.host,'supervisor');
    const version=String(initial.issue.version), key='chief-metadata-'+number;
    const args=['--supervisor','edit',number,'--if-version',version,'--label','reviewed','--remove-label','rework needed','--request-id',key];
    const result=await issue(peer,args);
    assert.deepEqual(result.issue.labels,['reviewed']);
    assert.deepEqual(await issue(peer,args),result);
    assert.deepEqual((await issue(main,['view',number])).issue,result.issue);
    assert.equal((await issue(peer,['--supervisor','edit',number,'--if-version',version,'--label','stale','--request-id','stale-'+number],4)).error.code,'conflict');
    check(number==='1'?'Closed unassigned cleanup, durable replay and stale guard':'Live assigned label edit, durable replay and stale guard');
  }
  assert.deepEqual(await ownership(),before);check('Issue ownership, running attempt and allocation unchanged');
  const file=join(root,'batch.json');
  const batch=[{number:1,if_version:3,expected_assignee:null,add_labels:['qualified']},{number:2,if_version:3,expected_assignee:'wrong',add_labels:['qualified']}];
  writeFileSync(file,JSON.stringify(batch));
  assert.equal((await issue(peer,['--supervisor','batch','--file',file,'--request-id','batch-stale'],4)).accepted,false);
  assert.deepEqual((await issue(main,['view','1'])).issue.labels,['reviewed']);check('Batch owner mismatch rolls back the entire group');
  batch[1].expected_assignee='codex:active-worker';writeFileSync(file,JSON.stringify(batch));
  assert.equal((await issue(peer,['--supervisor','batch','--file',file,'--request-id','batch-good'])).accepted,true);
  assert.deepEqual(await ownership(),before);check('Label-only batch preserves live ownership');
  const edit=['--supervisor','edit','2','--body','## Corrected guidance\n\nFollow the existing dependency graph.','--if-version','4','--request-id','body-correction'];
  assert.match((await issue(peer,edit)).issue.body,/Corrected guidance/);check('Guarded body correction is canonical');
  for(const args of [
    ['claim','2','--force'],['reopen','1','--if-version','4'],['edit','2','--draft','--request-id','draft'],
    ['edit','2','--label','unguarded','--request-id','unguarded'],['edit','2','--label','no-key','--if-version','5'],
  ])assert.equal((await issue(peer,['--supervisor',...args],2)).error.code,'invalid_input');
  batch[0].assignment='unassign';writeFileSync(file,JSON.stringify(batch));
  assert.equal((await issue(peer,['--supervisor','batch','--file',file,'--request-id','assignment'],2)).error.code,'invalid_input');
  check('Lifecycle, claim, assignment and missing guard/key rejected');
  assert.equal((await issue(peer,['--supervisor','edit','2','--label','yolo','--if-version','5','--request-id','yolo'],1)).error.code,'forbidden');
  check('Actor authorization retained');
  assert.deepEqual(await sql(main,"SELECT DISTINCT actor FROM requests WHERE request_id IN ('chief-metadata-1','chief-metadata-2','batch-good','body-correction')"),[['codex:chief-fixture']]);
  assert.deepEqual(await sql(peer,"SELECT count(*) FROM requests WHERE request_id IN ('chief-metadata-1','chief-metadata-2','batch-good','body-correction')"),[[0]]);
  check('Only authority stores receipts under the original actor');
  for(let attempt=0;;attempt++) {
    const local=(await issue(peer,['list','--state','all','--all'])).issues.find(i=>i.number===2);
    if(local && (await issue(peer,['view','2'])).issue.body.includes('Corrected guidance'))break;
    assert(attempt<200,'Replica did not converge');await wait(100);
  }
  assert.equal((await issue(peer,['edit','1','--label','offline'],4)).error.code,'fleet_allocation_missing');
  assert.deepEqual(await ownership(),before);check('Replica converges and ordinary offline allocation protection remains');
  const capabilities=await cli(peer,['fleet','capabilities']);
  assert.equal(capabilities.route,'supervisor_tunnel');
  assert.equal(capabilities.capabilities.issue_draft,true);
  assert.equal(capabilities.capabilities.issue_metadata,true);
  check('Companion discovers draft and metadata support without reverse SSH');
  await issue(main,['create','--title','Eligible import — paused as a draft','--body','## Imported task\n\nThe user requested that this eligible issue remain a draft. No worker or claim is needed.']);
  const draftArgs=['edit','3','--draft','--if-version','1'];
  const drafted=await issue(peer,draftArgs);
  assert.equal(drafted.issue.draft,true);assert.equal(drafted.store.host,'supervisor');
  assert.deepEqual(await issue(peer,draftArgs),drafted);
  assert.deepEqual((await issue(main,['view','3'])).issue,drafted.issue);
  check('Ordinary guarded drafting commits at authority and retries idempotently');
  const denied=await issue(peer,['edit','2','--draft','--if-version','5','--request-id','assigned-draft'],4);
  assert.equal(denied.error.code,'conflict');assert.match(denied.error.message,/unassigned, unreserved/);
  assert.equal((await issue(main,['view','2'])).issue.draft,false);
  check('Drafting assigned work is rejected at the authority');
  assert.equal((await issue(peer,['edit','3','--draft','--if-version','1','--request-id','stale-draft'],4)).error.code,'conflict');
  assert.equal((await issue(peer,['edit','3','--draft'],2)).error.code,'invalid_input');
  check('Draft edits require a current version');
  assert.deepEqual(await sql(main,"SELECT DISTINCT actor FROM requests WHERE request_id LIKE 'draft-%'"),[['codex:chief-fixture']]);
  assert.deepEqual(await sql(peer,"SELECT count(*) FROM requests WHERE request_id LIKE 'draft-%'"),[[0]]);
  assert.deepEqual(await sql(main,'SELECT count(*) FROM fleet_allocations WHERE issue_number=3'),[[0]]);
  check('Drafting retains actor identity and creates no reservation or replica receipt');
  await issue(main,['create','--title','Eligible import — browser draft action','--body','## Ready for triage\n\nMove this unassigned import to a draft from the connected companion.']);
  for(let attempt=0;;attempt++) {
    const issues=(await issue(peer,['list','--all'])).issues;
    if(issues.some(i=>i.number===3&&i.draft)&&issues.some(i=>i.number===4))break;
    assert(attempt<200,'Draft did not replicate');await wait(100);
  }
  check('Saved draft converges to the companion viewer');
  for(const path of [main,peer])assert.deepEqual(await sql(path,'PRAGMA integrity_check'),[['ok']]);
  check('Both issue stores pass integrity checks');
  assert.equal(checks.length,18,'Incomplete qualification graph');
  console.log(JSON.stringify({status:'passed',completed:checks.length,expected:18,checks}));
  if(serve) {
    const web=start(peer,['issue','--project','Chief metadata QA','--agent','codex:chief-fixture','--json','web','--port','59651','--no-discovery']);web.stdout.resume();
    for(let attempt=0;;attempt++) {
      try {if((await fetch('http://127.0.0.1:59651')).ok)break;}catch{}
      assert(attempt<600,'Viewer startup incomplete');await wait(50);
    }
    console.log(JSON.stringify({ready:true,pid:process.pid,root,url:'http://127.0.0.1:59651'}));
  } else await close();
}catch(error){
  console.error(error);
  try { console.error(JSON.stringify((await cli(main,['fleet','status'])).machines)); } catch {}
  process.exitCode=1;await close();
}
