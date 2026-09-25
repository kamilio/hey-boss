// Real supervisor/companion transport with synthetic maps. Ctrl+C cleans up.
import {mkdtempSync, mkdirSync, writeFileSync, chmodSync, copyFileSync, rmSync, existsSync, realpathSync} from 'node:fs';
import {spawn, spawnSync} from 'node:child_process';
import {join, resolve} from 'node:path';
import {createHash} from 'node:crypto';

const root=realpathSync(mkdtempSync('/tmp/hb-authority-'));
// Keep this run's executable stable while other checks rebuild target/debug.
// The viewer deliberately reloads when its executable is replaced.
const binary=join(root,'hey-boss');
const main=join(root,'main'), peer=join(root,'peer'), bin=join(root,'bin');
const children=[];
let closing=false;
let supervisor, changingConnection=false;
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
const cli=(path,args)=>new Promise((resolve,reject)=>{
  const child=start(path,args); let output='';
  child.stdout.on('data',chunk=>output+=chunk);
  child.once('error',reject);
  child.once('exit',(code,signal)=>code===0&&!signal?resolve(JSON.parse(output)):reject(Error(`CLI incomplete: code=${code}, signal=${signal}: ${output}`)));
});
const wait=ms=>new Promise(resolve=>setTimeout(resolve,ms));
function startSupervisor(){
  supervisor=start(main,['fleet','supervisor'],{PATH:`${bin}:${process.env.PATH}`});
  supervisor.stdout.resume();
}
// This signal controls only this fixture's owned supervisor, for outage/recovery QA.
process.on('SIGUSR1',async()=>{
  if(closing||changingConnection)return;
  changingConnection=true;
  try {
    if(supervisor&&supervisor.exitCode===null&&supervisor.signalCode===null){
      await new Promise(resolve=>{supervisor.once('exit',resolve);supervisor.kill('SIGTERM');});
      console.log('Fixture supervisor offline');
    }else{startSupervisor();console.log('Fixture supervisor reconnecting');}
  }finally{changingConnection=false;}
});
async function close(){
  if(closing)return;closing=true;
  for(const child of children.toReversed()) {
    if(child.exitCode!==null||child.signalCode!==null)continue;
    await new Promise(resolve=>{child.once('exit',resolve);child.kill('SIGTERM');});
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
  console.log('COMPLETE: authority fixture stopped; temporary files removed');
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
  const mm=args=>cli(main,['mm','--project','Fleet routing QA','--agent','human:fixture','--json',...args]);
  await mm(['add','Autumn release','--id','release','--body','A shared authoritative map, available from every connected device.']);
  for(const [id,title,body] of [['design','Design review','Review keyboard access, layout, and clear offline recovery.'],['implementation','Implementation','Preserve ownership, request IDs, and version guards.'],['verification','Verification','Run desktop and phone checks before publishing.']])
    await mm(['add',title,'--id',id,'--under','release','--body',body]);
  await mm(['link','verification','implementation','--kind','depends-on','--why','Validate the installed implementation']);
  await cli(main,['artifact','--project','Fleet routing QA','--agent','human:fixture','--json','create','--title','Review notes','--body','Verify attached resources through the same supervisor connection.','--node','design']);
  const attachment=join(main,'design.txt');
  writeFileSync(attachment,'Keyboard, layout, and offline recovery checklist.\n');
  await cli(main,['attachment','--project','Fleet routing QA','--agent','human:fixture','--json','upload',attachment,'--node','design']);
  writeFileSync(join(bin,'ssh'),'#!/bin/sh\nexport HEY_BOSS_ISSUE_DB="$AUTHORITY_PEER/issues.db" HEY_BOSS_FLEET_STATE="$AUTHORITY_PEER" HEY_BOSS_FLEET_CONFIG="$AUTHORITY_PEER/inventory.json" HEY_BOSS_FLEET_DESIRED="$AUTHORITY_PEER/desired.json"\n"$HEY_BOSS_TEST_CLI" fleet companion --stdio\nresult=$?; exit "$result"\n');
  chmodSync(join(bin,'ssh'),0o700);
  startSupervisor();
  for(let attempt=0;;attempt++) {
    try {await cli(peer,['fleet','status']);break;}
    catch(error){if(attempt===100)throw error;await wait(100);}
  }
  const web=start(peer,['mm','--project','Fleet routing QA','--agent','human:fixture','--json','web','--port','59645','--no-discovery']);web.stdout.resume();
  web.once('exit',(code,signal)=>{
    if(closing)return;
    console.error(`Fixture viewer exited unexpectedly: code=${code}, signal=${signal}`);
    process.exitCode=1;void close();
  });
  for(let attempt=0;;attempt++) {
    try {if((await fetch('http://127.0.0.1:59645/mm')).ok)break;}catch{}
    if(attempt===1200)throw Error('Companion viewer failed to start');await wait(50);
  }
  console.log(JSON.stringify({ready:true,pid:process.pid,supervisor_pid:supervisor.pid,root,url:'http://127.0.0.1:59645/mm'}));
}catch(error){console.error(error);process.exitCode=1;await close();}
