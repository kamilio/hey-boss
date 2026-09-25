// Disposable two-peer qualification of the installed CLI and native replica engine.
import assert from 'node:assert/strict';
import {mkdtempSync, mkdirSync, copyFileSync, realpathSync, existsSync, rmSync} from 'node:fs';
import {join, resolve} from 'node:path';
import {spawn, spawnSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {setTimeout as delay} from 'node:timers/promises';

const source = resolve(process.argv[2] || 'target/debug/hey-boss');
const serve = process.argv.includes('--serve');
const root = realpathSync(mkdtempSync('/tmp/hb-metadata-'));
const binary = join(root, 'hey-boss');
copyFileSync(source, binary);
const peers = ['main', 'peer'].map(name => join(root, name));
const children = [], checks = [];
let closing = false;
const envFor = path => {
  const env = {...process.env, HEY_BOSS_ISSUE_DB:join(path,'issues.db'), HEY_BOSS_FLEET_STATE:path,
    HEY_BOSS_INBOX_SOCKET:join(path,'inbox.sock')};
  for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID','HEY_BOSS_FLEET_SUPERVISED']) delete env[key];
  return env;
};
const run = (path, args, input, code=0) => {
  const result = spawnSync(binary,args,{env:envFor(path),cwd:path,input,encoding:'utf8',timeout:30000});
  assert.equal(result.signal,null,'Interrupted command: '+args.join(' '));
  assert.equal(result.status,code,result.stderr || result.stdout || String(result.error));
  return JSON.parse(result.stdout);
};
const issue = (path,args,code=0) => run(path,['issue','--project','Metadata sync QA','--agent','human:fixture','--json',...args],undefined,code);
const db = (path,request,ok=true) => {
  const result=run(path,['fleet','database','--path',join(path,'issues.db')],JSON.stringify(request)+'\n');
  assert.equal(result.ok,ok,JSON.stringify(result));
  return result;
};
const sql = (path,statement,args=[]) => db(path,{sql:statement,args}).rows;
const replica = (path,request) => JSON.parse(db(path,request).rows[0][0]);
const check = (ok,name) => {assert(ok,name);checks.push(name);};
const start = (path,args) => {
  const child=spawn(binary,args,{env:envFor(path),cwd:path,stdio:['ignore','pipe','inherit']});
  child.stdout.resume();children.push(child);return child;
};
async function cleanup() {
  if(closing)return;closing=true;
  for(const child of children.toReversed()) {
    if(child.exitCode!==null||child.signalCode!==null)continue;
    const result=await new Promise(resolve=>{child.once('exit',(code,signal)=>resolve({code,signal}));child.kill('SIGTERM');});
    assert.equal(result.code,0,'Fixture service did not exit normally');
    assert.equal(result.signal,null);
  }
  for(const path of peers) {
    const id=createHash('sha256').update(join(path,'issues.db')).digest('hex').slice(0,24);
    for(const suffix of ['.sock','.lock','.startup','.log'])rmSync(`/tmp/hey-boss-db-${process.getuid()}/${id}${suffix}`,{force:true});
  }
  rmSync(root,{recursive:true,force:true});
  console.log(JSON.stringify({cleanup_complete:true}));
}
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,()=>{cleanup().catch(error=>{console.error(error);process.exitCode=1;});});
try {
  const versionResult=spawnSync(binary,['--version'],{encoding:'utf8',timeout:30000});
  assert.equal(versionResult.status,0,'Version check failed');
  assert.equal(versionResult.signal,null,'Version check interrupted');
  const version=versionResult.stdout.trim();
  for(const path of peers) {
    mkdirSync(path);
    const child=start(path,['fleet','companion']);
    const id=createHash('sha256').update(join(path,'issues.db')).digest('hex').slice(0,24);
    for(let attempt=0;!existsSync(`/tmp/hey-boss-db-${process.getuid()}/${id}.sock`);attempt++) {
      assert(attempt<600&&child.exitCode===null&&child.signalCode===null,'Database startup incomplete');await delay(50);
    }
  }
  const [main,peer]=peers;
  replica(main,{replica:'capture',role:'controller',node:'main'});
  replica(peer,{replica:'capture',role:'agent',node:'peer'});
  issue(main,['create','--title','Metadata stays saved','--body','Verify labels, lifecycle, comments, and explicit recovery.']);
  issue(main,['close','1']);
  const snapshot=()=>replica(main,{replica:'snapshot',node:'peer'});
  const pull=(payload,receipts=[])=>replica(peer,{replica:'pull',node:'peer',payload,receipts});
  const journal=()=>sql(peer,'SELECT * FROM fleet_outbox ORDER BY seq').map(row=>Object.fromEntries(['seq','table_name','before_json','after_json','created_at'].map((key,i)=>[key,row[i]])));
  pull(snapshot());
  const before=issue(peer,['view','1']);
  for(const args of [['edit','1','--label','rework','--request-id','label-once'],['reopen','1','--if-version','2','--request-id','reopen-once']]) {
    const failure=issue(peer,args,4);
    check(failure.error.code==='fleet_allocation_missing'&&failure.error.message.includes('not saved'),'Unallocated '+args[0]+' fails before acceptance');
    assert.deepEqual(issue(peer,['view','1']),before);
  }
  check(journal().length===0,'Rejected mutations leave no journal entries');
  check(sql(peer,'SELECT count(*) FROM requests')[0][0]===0,'Rejected request IDs are not cached');
  // Allocation is synthetic fixture setup, never a production reservation.
  sql(main,"INSERT INTO fleet_allocations VALUES('named:Metadata sync QA',1,'peer')");
  pull(snapshot());
  const sync=()=>{
    const changes=journal();
    const receipts=replica(main,{replica:'accept',node:'peer',changes});
    assert(receipts.every(r=>r.state==='applied'),JSON.stringify(receipts));
    assert.deepEqual(replica(main,{replica:'accept',node:'peer',changes}),receipts);
    const previous=JSON.parse(sql(peer,"SELECT value FROM fleet_state WHERE key='cursor'")[0][0]);
    const payload=replica(main,{replica:'incremental',node:'peer',cursor:previous});
    assert(payload.cursor>previous,'Sync cursor did not advance');
    pull(payload,receipts);
    assert.equal(journal().length,0,'Journal did not drain');
    for(const query of ['SELECT title,body,labels,state,version FROM issues','SELECT body FROM comments ORDER BY body','SELECT action,data FROM events ORDER BY action,data']) {
      // Comment IDs are replica-local; compare event counts separately below.
      if(query.includes('events'))continue;
      assert.deepEqual(sql(peer,query),sql(main,query));
    }
    assert.deepEqual(sql(peer,'SELECT action,count(*) FROM events GROUP BY action'),sql(main,'SELECT action,count(*) FROM events GROUP BY action'));
    return payload;
  };
  const label=['edit','1','--label','rework','--request-id','label-once'];
  const accepted=issue(peer,label);
  assert.deepEqual(issue(peer,label),accepted);
  issue(main,['edit','1','--title','Concurrent title survives']);
  issue(main,['comment','1','--body','Supervisor finding']);
  issue(peer,['comment','1','--body','Companion finding']);
  sync();check(true,'Concurrent fields, comments, audit events and duplicate requests converge');
  const stale=snapshot();
  const revision=String(issue(peer,['view','1']).issue.version);
  issue(peer,['reopen','1','--if-version',revision,'--request-id','reopen-once']);
  sync();check(true,'Guarded reopen converges with advancing cursor and drained journal');
  const stable=issue(peer,['view','1']);
  const rejected=db(peer,{replica:'pull',node:'peer',payload:stale,receipts:[]},false);
  check(rejected.error.includes('Stale fleet pull'),'Stale snapshot is explicitly rejected');
  assert.deepEqual(issue(peer,['view','1']),stable);
  issue(main,['edit','1','--label','later']);
  pull(snapshot());
  check(issue(peer,['view','1']).issue.labels.includes('later'),'Legitimate later metadata still applies');
  issue(peer,['close','1','--comment','Verified completion']);sync();
  check(issue(peer,['view','1']).issue.state==='closed','Offline close and comment converge');
  check(sql(peer,'PRAGMA integrity_check')[0][0]==='ok'&&sql(main,'PRAGMA integrity_check')[0][0]==='ok','Both replicas pass integrity checks');
  assert.equal(checks.length,10,'Incomplete qualification graph');
  console.log(JSON.stringify({status:'passed',version,completed:checks.length,expected:10,checks}));
  if(serve) {
    sql(main,'DELETE FROM fleet_allocations');pull(snapshot());
    start(peer,['issue','--project','Metadata sync QA','web','--port','59650','--no-discovery','--json']);
    console.log(JSON.stringify({ready:true,pid:process.pid,url:'http://127.0.0.1:59650',root}));
  } else await cleanup();
} catch(error) {console.error(error);process.exitCode=1;await cleanup();}
