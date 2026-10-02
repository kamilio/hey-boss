// Exercise project setup through a real supervisor, isolated from the user's fleet.
import assert from 'node:assert/strict';
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,existsSync,rmSync,chmodSync,realpathSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawn,spawnSync} from 'node:child_process';
import {createConnection} from 'node:net';
const root=realpathSync(mkdtempSync(join(tmpdir(),'hb-project-checks-')));
const binary=resolve(process.env.HEY_BOSS_TEST_BINARY||'target/debug/hey-boss');
const project='github.com/acme/checkout-fixture',url='https://'+project+'.git';
const env={...process.env,HOME:root,HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_FLEET_STATE:root,HEY_BOSS_FLEET_DESIRED:join(root,'fleet.yaml'),HEY_BOSS_FLEET_CONFIG:join(root,'inventory.json'),HEY_BOSS_FLEET_BINARY:binary,GIT_CONFIG_GLOBAL:join(root,'gitconfig'),GIT_CONFIG_NOSYSTEM:'1'};
delete env.HEY_BOSS_ISSUE_HOST;delete env.HEY_BOSS_ISSUE_PROJECT;
const realGit=spawnSync('which',['git'],{encoding:'utf8'}).stdout.trim();
function git(args){const r=spawnSync(realGit,args,{env,encoding:'utf8'});assert.equal(r.status,0,r.stderr);}
function rpc(value){return new Promise((resolve,reject)=>{let data='';const socket=createConnection(join(root,'fleet.sock'));socket.setTimeout(10000,()=>socket.destroy(Error('RPC timeout')));socket.on('connect',()=>socket.write(JSON.stringify(value)+'\n'));socket.on('data',chunk=>{data+=chunk;if(data.includes('\n')){socket.end();try{resolve(JSON.parse(data.trim()));}catch(e){reject(e);}}});socket.on('error',reject);socket.on('end',()=>{if(data&&!data.includes('\n')){try{resolve(JSON.parse(data));}catch(e){reject(e);}}});});}
async function until(fn,label,seconds=25){const deadline=Date.now()+seconds*1000;while(Date.now()<deadline){const result=await fn();if(result)return result;await new Promise(r=>setTimeout(r,250));}throw Error(label);}
async function machine(){return(await rpc({kind:'status'})).machines.find(m=>m.host==='local');}
async function edit(path,git=url){const c=await rpc({kind:'configuration'});const result=await rpc({kind:'configuration',revision:c.revision,save:true,machine_update:{host:'local',action:'project',git,workspace:join(root,'Workspace'),...(path?{path}:{})}});assert.equal(result.ok,true,JSON.stringify(result));return result;}
async function applied(path,seconds){return until(async()=>{const m=await machine();return !m.configuration_error&&m.applied_revision===m.desired_revision&&m.projects[project]?.resolved_path===path;},'Project did not apply at '+path,seconds);}
let supervisor;
try{
  const source=join(root,'source');mkdirSync(source);git(['init','--quiet',source]);writeFileSync(join(source,'README.md'),'fixture\n');git(['-C',source,'add','.']);git(['-C',source,'-c','user.name=Fixture','-c','user.email=fixture@example.invalid','-c','commit.gpgsign=false','commit','--quiet','-m','Fixture']);
  writeFileSync(env.GIT_CONFIG_GLOBAL,'[url "'+source+'"]\n\tinsteadOf = '+url+'\n');
  mkdirSync(join(root,'bin'));const wrapper=join(root,'bin/git');
  // The SSH failure and slow clone run in the service process, not the test client.
  writeFileSync(wrapper,`#!/bin/sh\ncase " $* " in\n*" clone "*)\n  printf '%s\\n' "$*" >> "$HOME/clone-calls"\n  case "$*" in *git@github.com:*) echo 'Permission denied (publickey).' >&2; exit 128;; esac\n  if test -f "$HOME/slow-clone"; then sleep 61; fi\n;;\nesac\nexec '${realGit}' "$@"\n`);chmodSync(wrapper,0o755);env.PATH=join(root,'bin')+':'+env.PATH;
  writeFileSync(env.HEY_BOSS_FLEET_DESIRED,'machines: {local: {workers: []}}\n');writeFileSync(env.HEY_BOSS_FLEET_CONFIG,'{"ssh_hosts":[]}');
  supervisor=spawn(binary,['fleet','supervisor'],{env,stdio:['ignore','ignore','pipe']});let errors='';supervisor.stderr.on('data',b=>errors+=b);
  await until(()=>existsSync(join(root,'fleet.sock')),'Supervisor failed: '+errors);
  const existing=join(root,'checkout-fixture');git(['clone','--quiet',url,existing]);writeFileSync(join(existing,'local-work'),'keep');
  await edit();await applied(existing);assert(!existsSync(join(root,'clone-calls')));assert.equal(readFileSync(join(existing,'local-work'),'utf8'),'keep');
  console.log('PASS automatic add reuses a matching checkout without cloning or modifying local work');
  const separate=join(root,'separate-clone');await edit(separate);await applied(separate);assert(existsSync(join(separate,'README.md')));assert(existsSync(join(existing,'local-work')));
  console.log('PASS explicit paths create independent clones despite a matching existing checkout');
  const wrong=join(root,'wrong');mkdirSync(wrong);git(['init','--quiet',wrong]);git(['-C',wrong,'remote','add','origin','https://github.com/other/repository.git']);writeFileSync(join(wrong,'keep'),'keep');
  await edit(wrong);await until(async()=>(await machine()).configuration_error?.includes('another repository'),'Wrong origin was accepted');assert.equal(readFileSync(join(wrong,'keep'),'utf8'),'keep');
  git(['-C',wrong,'remote','set-url','origin',url]);const retry=await rpc({kind:'configuration',retry_project:{host:'local',project}});assert.equal(retry.ok,true);await applied(wrong);
  console.log('PASS wrong repository is preserved and explicit retry clears the failure after correction');
  const fallback=join(root,'ssh-fallback');await edit(fallback,'git@github.com:acme/checkout-fixture.git');await applied(fallback);assert(existsSync(join(fallback,'README.md')));const calls=readFileSync(join(root,'clone-calls'),'utf8');assert(calls.includes('git@github.com:acme/checkout-fixture.git'));assert(calls.includes(url));
  console.log('PASS background SSH authentication failure retries the same repository through HTTPS');
  const slow=join(root,'slow');writeFileSync(join(root,'slow-clone'),'');await edit(slow);await applied(slow,90);assert(existsSync(join(slow,'README.md')));
  console.log('PASS a clone taking over 60 seconds completes without a false timeout');
}finally{
  if(supervisor){supervisor.kill('SIGTERM');await Promise.race([new Promise(r=>supervisor.once('exit',r)),new Promise(r=>setTimeout(r,3000))]);}
  rmSync(root,{recursive:true,force:true});
}
