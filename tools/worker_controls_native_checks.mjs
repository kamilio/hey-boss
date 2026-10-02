// Full native lifecycle and clone check. Uses a private store and local Git source.
import assert from 'node:assert/strict';
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,existsSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawn,spawnSync} from 'node:child_process';
import {createConnection} from 'node:net';
const root=mkdtempSync(join(tmpdir(),'hb-worker-controls-'));
const binary=resolve(process.env.HEY_BOSS_TEST_BINARY||'target/debug/hey-boss');
const children=[],workerPids=new Set();
const env={...process.env,HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_FLEET_STATE:root,HEY_BOSS_FLEET_DESIRED:join(root,'fleet.yaml'),HEY_BOSS_FLEET_CONFIG:join(root,'inventory.json'),HEY_BOSS_FLEET_BINARY:binary,GIT_CONFIG_GLOBAL:join(root,'gitconfig')};
delete env.HEY_BOSS_ISSUE_HOST;delete env.HEY_BOSS_ISSUE_PROJECT;
function git(args){const r=spawnSync('git',args,{encoding:'utf8',env});assert.equal(r.status,0,r.stderr);}
function rpc(value){return new Promise((resolve,reject)=>{let data='';const socket=createConnection(join(root,'fleet.sock'));socket.setTimeout(15000,()=>socket.destroy(Error('Fixture request timed out')));socket.on('connect',()=>socket.write(JSON.stringify(value)+'\n'));socket.on('data',chunk=>{data+=chunk;if(data.includes('\n')){socket.end();try{resolve(JSON.parse(data.trim()));}catch(error){reject(error);}}});socket.on('error',reject);socket.on('end',()=>{if(data&&!data.includes('\n')){try{resolve(JSON.parse(data));}catch(error){reject(error);}}});});}
async function until(fn,label){for(let n=0;n<100;n++){const result=await fn();if(result)return result;await new Promise(r=>setTimeout(r,200));}throw Error(label);}
async function edit(update){const config=await rpc({kind:'configuration'});const result=await rpc({kind:'configuration',revision:config.revision,machine_update:update,save:true});assert.equal(result.ok,true,JSON.stringify(result));return result;}
try{
  writeFileSync(env.HEY_BOSS_FLEET_DESIRED,'machines: {local: {workers: []}}\n');writeFileSync(env.HEY_BOSS_FLEET_CONFIG,'{"ssh_hosts":[]}');
  const source=join(root,'source');mkdirSync(source);git(['init','--quiet',source]);writeFileSync(join(source,'README.md'),'Synthetic checkout\n');git(['-C',source,'add','README.md']);git(['-C',source,'-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','--quiet','-m','Initial fixture']);
  writeFileSync(env.GIT_CONFIG_GLOBAL,'[url "'+join(root,'unavailable-source')+'"]\n\tinsteadOf = https://github.com/acme/worker-fixture.git\n');
  const supervisor=spawn(binary,['fleet','supervisor'],{env,stdio:['ignore','pipe','pipe']});children.push(supervisor);let output='';supervisor.stdout.on('data',b=>output+=b);supervisor.stderr.on('data',b=>output+=b);
  await until(()=>existsSync(join(root,'fleet.sock')),'Supervisor startup failed: '+output);
  const project='github.com/acme/worker-fixture',workspace=join(root,'Workspace'),checkout=join(workspace,'worker-fixture');
  const saved=await edit({host:'local',action:'project',git:'https://github.com/acme/worker-fixture.git',workspace});
  assert.equal(saved.document.machines.local.workspace,workspace);
  await until(async()=>{const s=await rpc({kind:'status'});return s.machines?.find(m=>m.host==='local')?.configuration_error;},'Failed clone did not report setup error');
  writeFileSync(env.GIT_CONFIG_GLOBAL,'[url "'+source+'"]\n\tinsteadOf = https://github.com/acme/worker-fixture.git\n');
  const retry=await rpc({kind:'configuration',retry_project:{host:'local',project}});assert.equal(retry.ok,true,JSON.stringify(retry));
  await until(()=>existsSync(join(checkout,'README.md')),'Explicit retry did not clone the project');
  await until(async()=>{const s=await rpc({kind:'status'});const m=s.machines?.find(m=>m.host==='local');return m&&!m.configuration_error&&!Object.keys(m.project_retries||{}).length;},'Successful retry did not clear setup error and pending status');
  assert.equal(readFileSync(join(checkout,'README.md'),'utf8'),'Synthetic checkout\n');
  await edit({host:'local',action:'add',id:'synthetic-worker',concurrency:1});
  const running=await until(async()=>{const s=await rpc({kind:'status'});const w=s.machines?.find(m=>m.host==='local')?.workers?.find(w=>w.id==='synthetic-worker');if(w?.pid>0){workerPids.add(w.pid);return w;}},'Worker did not start');
  assert.deepEqual(running.config.projects,[project]);assert.equal(running.config.directories[project],checkout);
  writeFileSync(join(checkout,'local-change.txt'),'Preserve user files');
  await edit({host:'local',action:'project',git:'https://github.com/acme/worker-fixture.git',workspace,worker:'synthetic-worker'});
  assert.equal(readFileSync(join(checkout,'local-change.txt'),'utf8'),'Preserve user files');
  await edit({host:'local',action:'remove',id:'synthetic-worker'});
  await until(async()=>{const s=await rpc({kind:'status'});const w=s.machines?.find(m=>m.host==='local')?.workers?.find(w=>w.id==='synthetic-worker');return w?.retiring&&!w?.pid;},'Graceful removal did not stop the idle worker');
  workerPids.clear();
  const config=await rpc({kind:'configuration'});assert.equal(config.document.machines.local.workers[0].intent,'drain');assert.equal(config.document.machines.local.workers[0].retiring,true);
  console.log('Native clone, saved defaults, worker startup, checkout assignment, existing-file preservation and graceful removal passed.');
}finally{
  for(const pid of workerPids){try{process.kill(pid,'SIGTERM');}catch{}}
  for(const child of children){child.kill('SIGTERM');await Promise.race([new Promise(r=>child.once('exit',r)),new Promise(r=>setTimeout(r,3000))]);if(child.exitCode===null)child.kill('SIGKILL');}
  // The private database broker may outlive its clients. Stop only this fixture's broker.
  const ps=spawnSync('ps',['-axo','pid=,command='],{encoding:'utf8'});
  for(const line of (ps.stdout||'').split('\n')){if(line.includes(root)&&line.includes('database')){const pid=Number(line.trim().split(/\s+/)[0]);if(pid&&pid!==process.pid)try{process.kill(pid,'SIGTERM');}catch{}}}
  rmSync(root,{recursive:true,force:true});
}
