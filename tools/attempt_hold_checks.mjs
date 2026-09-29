// Disposable installed-CLI qualification. --serve also opens a native visual fixture.
import assert from 'node:assert/strict';
import {mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, copyFileSync, existsSync, realpathSync} from 'node:fs';
import {spawn, spawnSync} from 'node:child_process';
import {resolve, join} from 'node:path';
import {createHash} from 'node:crypto';
const root=realpathSync(mkdtempSync('/tmp/hb-attempt-qa-'));
const binary=join(root,'hey-boss'), source=join(root,'retained-source');
const children=[];let closing=false, server;
const serve=process.argv.includes('--serve');
const env={...process.env,HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:join(root,'inbox.sock'),HEY_BOSS_AGENT_ID:'codex:attempt-qa'};
for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID','CLAUDE_SESSION_ID'])delete env[key];
const start=(command,args,options={})=>{const child=spawn(command,args,{env,cwd:source,stdio:['pipe','pipe','pipe'],...options});children.push(child);return child;};
const run=(args,expected=0)=>new Promise((resolve,reject)=>{
 const child=start(binary,['issue','--project','Surviving attempt QA','--json',...args]);let stdout='',stderr='';
 child.stdout.on('data',c=>stdout+=c);child.stderr.on('data',c=>stderr+=c);child.on('error',reject);
 const timer=setTimeout(()=>{child.kill('SIGTERM');reject(Error('CLI timeout: '+args));},30000);
 child.on('exit',code=>{clearTimeout(timer);try{assert.equal(code,expected,stdout+stderr);resolve(JSON.parse(stdout));}catch(e){reject(e);}});
});
const wait=ms=>new Promise(r=>setTimeout(r,ms));
const socketBase='/tmp/hey-boss-db-'+process.getuid()+'/'+createHash('sha256').update(env.HEY_BOSS_ISSUE_DB).digest('hex').slice(0,24);
async function close(){
 if(closing)return;closing=true;
 for(const child of [...children].reverse())if(child.exitCode===null&&child.signalCode===null)await new Promise(resolve=>{child.once('exit',resolve);child.kill('SIGTERM');});
 rmSync(root,{recursive:true,force:true});
 for(const suffix of ['.sock','.lock','.startup','.log'])rmSync(socketBase+suffix,{force:true});
 console.log('CLEAN: fixture processes and temporary files removed');
}
for(const signal of ['SIGINT','SIGTERM'])process.on(signal,()=>void close());
try{
 copyFileSync(resolve(process.env.HEY_BOSS_TEST_BINARY||'target/debug/hey-boss'),binary);mkdirSync(source);
 for(const args of [['init','-q'],['-c','user.name=QA','-c','user.email=qa@example.test','commit','--allow-empty','-qm','Fixture']])assert.equal(spawnSync('git',args,{cwd:source}).status,0);
 const owner=start(binary,['fleet','companion']);owner.stdout.resume();owner.stderr.on('data',c=>process.stderr.write(c));
 for(let n=0;!existsSync(socketBase+'.sock');n++){assert(n<400&&owner.exitCode===null,'Database startup');await wait(50);}
 for(const code of [0,7]){
  const created=await run(['create','--title',code?'Retained validation failure':'Retained validation success']);const number=created.issue.number;
  let value=await run(['claim',String(number)]);
  const child=start('/bin/sh',['-c','read outcome; exit "$outcome"'],{detached:true});
  const log=join(root,'validation-'+number+'.log');writeFileSync(log,'validation started\n');
  const report=join(root,'attempt.json');writeFileSync(report,JSON.stringify({attempt_id:'validation-'+number,owner:'codex:attempt-qa',pid:child.pid,log_path:log,worktree:source}));
  const version=value.issue.version;
  value=await run(['attempt','hold',String(number),'--if-version',String(version),'--file',report]);
  assert.equal(value.issue.attempt_hold.pid,child.pid);
  assert.equal((await run(['attempt','hold',String(number),'--if-version',String(version),'--file',report],4)).ok,false);
  value=await run(['unassign',String(number)]);
  assert.equal((await run(['claim',String(number),'--force'],1)).error.code,'attempt_held');
  let inspected=await run(['attempt','inspect',String(number)]);assert.equal(inspected.evidence.process,'live');
  const evidence=join(root,'evidence.json');writeFileSync(evidence,JSON.stringify(inspected));
  assert.equal((await run(['attempt','reconcile',String(number),'--if-version',String(value.issue.version),'--file',evidence,'--outcome','Reviewed'],4)).ok,false);
  await new Promise(resolve=>{child.once('exit',resolve);child.stdin.end(code+'\n');});assert.equal(child.exitCode,code);
  writeFileSync(log,'validation exited '+code+'\n');inspected=await run(['attempt','inspect',String(number)]);assert.equal(inspected.evidence.process,'terminal');
  writeFileSync(evidence,JSON.stringify(inspected));
  value=await run(['attempt','reconcile',String(number),'--if-version',String(inspected.issue.version),'--file',evidence,'--outcome','Reviewed exit '+code+'; retained source has no changes.']);
  assert.equal(value.issue.attempt_hold,null);await run(['claim',String(number)]);
  console.log('PASS: installed CLI protects a live attempt and reconciles exit '+code);
 }
 if(serve){
  let value=await run(['create','--title','Protect retained validation while its agent is unavailable']);const number=value.issue.number;value=await run(['claim',String(number)]);
  const child=start('/bin/sh',['-c','read outcome; exit "$outcome"'],{detached:true});
  const log=join(root,'validation-with-a-long-descriptive-name-for-layout-testing.log');writeFileSync(log,'validation waiting\n');
  const report=join(root,'visual-attempt.json');writeFileSync(report,JSON.stringify({attempt_id:'validation-wrapper-62791',owner:'codex:attempt-qa',pid:child.pid,log_path:log,worktree:source}));
  await run(['attempt','hold',String(number),'--if-version',String(value.issue.version),'--file',report]);await run(['unassign',String(number)]);
  server=start(binary,['issue','--project','Surviving attempt QA','web','--port','59673','--no-discovery']);server.stdout.resume();server.stderr.on('data',c=>process.stderr.write(c));
  console.log(JSON.stringify({pid:process.pid,root,url:'http://127.0.0.1:59673/#project=named%3ASurviving%20attempt%20QA&issue='+number,number}));
 }else await close();
}catch(error){console.error(error);process.exitCode=1;await close();}
