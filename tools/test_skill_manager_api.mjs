// Real HTTP jobs, isolated fleet homes, and a failed connection; no user files.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, writeFile, readFile, chmod, stat, access, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {spawn} from 'node:child_process';
const binary=resolve(process.env.HEY_BOSS_TEST_BINARY||'target/debug/hey-boss');
const root=await mkdtemp(join(tmpdir(),'skill-manager-test-'));
const local=join(root,'local'), remote=join(root,'studio');
const put=async(path,content)=>{await mkdir(resolve(path,'..'),{recursive:true});await writeFile(path,content);};
const exists=path=>access(path).then(()=>true,()=>false);
let server,origin,csrf,log='';
const stop=async()=>{if(server?.exitCode===null){server.kill();await new Promise(r=>server.once('exit',r));}};
try{
  for(const [host,home] of [['local',local],['studio',remote]]){
    await put(join(home,'.hey-boss/skill-sync.json'),'{"selected":[]}');
    await put(join(home,'.claude/skills/stacked-prs/SKILL.md'),`---\nname: stacked-prs\ndescription: Fixture\n---\n${host} version\n`);
    await put(join(home,'.codex/AGENTS.md'),host==='local'?'# Fixture instructions\n':'');
  }
  const remoteSkill=join(remote,'.agents/skills/remote-only');
  await put(join(remoteSkill,'SKILL.md'),'---\nname: remote-only\ndescription: Fixture\n---\nRun the helper.');
  await put(join(remoteSkill,'scripts/nested/run.sh'),'#!/bin/sh\necho hello\n');
  await chmod(join(remoteSkill,'scripts/nested/run.sh'),0o755);
  await put(join(remoteSkill,'references/notes.md'),'# Companion notes\n');
  const bytes=Buffer.from(Array.from({length:256},(_,i)=>i));
  await put(join(remoteSkill,'asset.bin'),bytes);
  await put(join(remote,'.claude/skills/bad-junk/SKILL.md'),'---\nname: bad-junk\n---\nFixture');
  await put(join(local,'.hey-boss/config.json'),'{"ssh_hosts":["studio","offline"]}');
  // Execute the tested remote binary directly; a login shell must not pick the developer's installed build.
  const shim=join(root,'bin/ssh');
  await put(shim,`#!/usr/bin/env node
const {spawnSync}=require('node:child_process');
const args=process.argv.slice(2),command=args.at(-1);
if(args.includes('offline')){console.error('Offline test machine');process.exit(255);}
const result=command.includes('skill apply-')?spawnSync(${JSON.stringify(binary)},['skill',command.includes('apply-copy')?'apply-copy':'apply-rename'],{stdio:'inherit',env:{...process.env,HOME:${JSON.stringify(remote)}}}):spawnSync('python3',[${JSON.stringify(resolve('src/skill/transport.py'))}],{stdio:'inherit',env:{...process.env,HOME:${JSON.stringify(remote)}}});
process.exit(result.status??1);
`);
  await chmod(shim,0o755);
  const env={...process.env,HOME:local,HEY_BOSS_STATE_DIR:join(root,'state'),PATH:`${join(root,'bin')}:${process.env.PATH}`};
  for(const key of ['HEY_BOSS_FLEET_CONFIG','HEY_BOSS_FLEET_STATE','HEY_BOSS_FLEET_DESIRED','HEY_BOSS_ISSUE_HOST'])delete env[key];
  const get=async(path='/api/skills')=>(await fetch(origin+path)).json();
  const start=async()=>{
    origin=undefined;log='';server=spawn(binary,['issue','web','--port','0','--no-discovery','--json'],{cwd:local,env});
    server.stdout.on('data',c=>log+=c);server.stderr.on('data',c=>log+=c);
    for(let n=0;n<300&&!origin;n++){
      for(const line of log.split('\n')){try{origin=JSON.parse(line).url||origin;}catch{}}
      if(server.exitCode!==null)throw Error(log);
      if(!origin)await new Promise(r=>setTimeout(r,100));
    }
    assert.ok(origin,log);origin=origin.replace(/\/$/,'');csrf=(await get('/api/bootstrap')).csrf;
  };
  const post=async(body,status=200,token=csrf)=>{
    const res=await fetch(origin+'/api/skills',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':token},body:JSON.stringify(body)});
    const value=await res.json();assert.equal(res.status,status,JSON.stringify(value));return value;
  };
  const idle=async()=>{for(let n=0;n<300;n++){const value=await get();if(!value.busy)return value;await new Promise(r=>setTimeout(r,100));}throw Error('Job stayed busy');};
  await start();await post({action:'scan'},403,'');
  assert.ok((await post({action:'scan'})).busy);let report=await idle();
  const machine=host=>report.machines.find(m=>m.host===host);
  assert.deepEqual(report.machines.map(m=>m.host).sort(),['local','offline','studio']);
  assert.equal(machine('offline').state,'attention');
  assert.ok(machine('studio').copies.every(c=>!('files' in c)));
  const source=machine('studio').copies.find(c=>c.name==='stacked-prs');
  const remoteCopy=machine('studio').copies.find(c=>c.name==='remote-only');
  assert.deepEqual(remoteCopy.markdown_files.map(f=>f.path),['SKILL.md','references/notes.md']);
  assert.ok(remoteCopy.ignored_files.includes('scripts/nested/run.sh'));
  const payload={action:'distribute',revision:report.revision,selected:['stacked-prs','remote-only'],choices:{},max_words:300};
  await post(payload,409);payload.choices={'stacked-prs':source.digest};payload.revision--;
  await post(payload,409);assert.deepEqual((await get()).selected,['hey-boss']);payload.revision=report.revision;
  await post(payload);report=await idle();assert.equal(report.max_words,300);
  for(const home of [local,remote])for(const agent of ['.codex','.agents','.claude','.hey-boss']){
    assert.match(await readFile(join(home,agent,'skills/stacked-prs/SKILL.md'),'utf8'),/studio version/);
    assert.deepEqual(await readFile(join(home,agent,'skills/remote-only/asset.bin')),bytes);
    assert.ok((await stat(join(home,agent,'skills/remote-only/scripts/nested/run.sh'))).mode&0o111);
  }
  const changed=join(local,'.claude/skills/stacked-prs/SKILL.md');
  await writeFile(changed,'Edited after scanning. Keep this.');payload.revision=report.revision;
  await post(payload);report=await idle();assert.equal(await readFile(changed,'utf8'),'Edited after scanning. Keep this.');
  assert.match(machine('local').error,/changed since/);
  await stop();await start();report=await get();assert.equal(report.choices['stacked-prs'],source.digest);
  assert.ok(report.selected.includes('remote-only'));assert.equal(report.busy,false);
  const instructions=machine('local').copies.find(c=>c.name==='AGENTS.md');
  await post({action:'distribute',revision:report.revision,skill:'AGENTS.md',selected:['AGENTS.md'],choices:{'AGENTS.md':instructions.digest},max_words:400});
  report=await idle();
  for(const home of [local,remote])for(const path of ['.codex/AGENTS.md','.agents/AGENTS.md','.claude/CLAUDE.md','.hey-boss/skills/AGENTS.md/SKILL.md']){
    assert.equal(await readFile(join(home,path),'utf8'),'# Fixture instructions\n');
  }
  await post({action:'save_file',skill:'remote-only',file_path:'references/notes.md',content:'# Updated companion notes\n',base_digest:remoteCopy.digest,distribute:true});
  report=await idle();
  for(const home of [local,remote])for(const agent of ['.codex','.agents','.claude','.hey-boss']){
    assert.equal(await readFile(join(home,agent,'skills/remote-only/references/notes.md'),'utf8'),'# Updated companion notes\n');
  }
  const opened=await post({action:'open_dir',skill:'remote-only'});
  assert.ok(await exists(opened.opened_dir));assert.ok(await exists(opened.opened_file));
  await post({action:'delete',skill:'bad-junk'});report=await idle();
  assert.equal(await exists(join(remote,'.claude/skills/bad-junk')),false);
  assert.ok(report.machines.every(m=>m.copies.every(c=>c.name!=='bad-junk')));
  console.log('Skills API: fleet failures, guarded distribution, backups, restart, instructions, reference edits, and deletion passed');
}finally{await stop();await rm(root,{recursive:true,force:true});}
