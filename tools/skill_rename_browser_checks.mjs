// Real HTTP and browser checks, using disposable homes instead of installed skills.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, writeFile, readFile, access, rm, chmod} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {spawn} from 'node:child_process';
const binary=resolve(process.env.HEY_BOSS_TEST_BINARY||'target/debug/hey-boss');
const root=await mkdtemp(join(tmpdir(),'skill-rename-'));
const local=join(root,'local'), remote=join(root,'remote');
const text=name=>`---\nname: ${name}\ndescription: Fixture\n---\n[Reference](references/notes.md)\n`;
const put=async(path,content)=>{await mkdir(resolve(path,'..'),{recursive:true});await writeFile(path,content);};
const exists=async path=>access(path).then(()=>true,()=>false);
let server,browser,log='';
try{
  for(const home of [local,remote]){
    await put(join(home,'.codex/skills/rename-before/SKILL.md'),text('rename-before'));
    await put(join(home,'.codex/skills/rename-before/references/notes.md'),'# Reference\n');
    await put(join(home,'.codex/skills/rename-before/helper.sh'),'#!/bin/sh\nexit 0\n');
    await chmod(join(home,'.codex/skills/rename-before/helper.sh'),0o755);
    await put(join(home,'.hey-boss/skill-sync.json'),JSON.stringify({selected:['rename-before']}));
  }
  await put(join(local,'.hey-boss/config.json'),JSON.stringify({ssh_hosts:['fixture-remote']}));
  const shim=join(root,'bin/ssh');
  await put(shim,`#!/usr/bin/env node\nconst {spawnSync}=require('node:child_process');\nconst args=process.argv.slice(2);\nconst command=args.at(-1);\nconst result=command.includes('skill apply-rename')?spawnSync(${JSON.stringify(binary)},['skill','apply-rename'],{stdio:'inherit',env:{...process.env,HOME:${JSON.stringify(remote)}}}):spawnSync('python3',[${JSON.stringify(resolve('src/skill/transport.py'))}],{stdio:'inherit',env:{...process.env,HOME:${JSON.stringify(remote)}}});\nif(result.status!==0)console.error({status:result.status,signal:result.signal,error:result.error});\nprocess.exit(result.status??1);\n`);
  await chmod(shim,0o755);
  const env={...process.env,HOME:local,PATH:`${join(root,'bin')}:${resolve(binary,'..')}:${process.env.PATH}`,HEY_BOSS_STATE_DIR:join(root,'state')};
  for(const key of ['HEY_BOSS_FLEET_CONFIG','HEY_BOSS_FLEET_STATE','HEY_BOSS_FLEET_DESIRED','HEY_BOSS_ISSUE_HOST'])delete env[key];
  let origin,csrf;
  const start=async()=>{
  log='';origin=undefined;
  server=spawn(binary,['issue','web','--port','0','--no-discovery','--json'],{cwd:local,env});
  server.stdout.on('data',chunk=>log+=chunk);server.stderr.on('data',chunk=>log+=chunk);
  for(let n=0;n<300&&!origin;n++){
    for(const line of log.split('\n')){try{origin=JSON.parse(line).url||origin;}catch{}}
    if(server.exitCode!==null)throw Error(log);
    if(!origin)await new Promise(r=>setTimeout(r,100));
  }
  assert.ok(origin,log);origin=origin.replace(/\/$/,'');
  const get=async path=>(await fetch(origin+path)).json();
  csrf=(await get('/api/bootstrap')).csrf;
  };
  const get=async path=>(await fetch(origin+path)).json();
  await start();
  const post=async (body,ok=true)=>{
    const res=await fetch(origin+'/api/skills',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':csrf},body:JSON.stringify(body)});
    const data=await res.json();assert.equal(res.ok,ok,JSON.stringify(data));return data;
  };
  const idle=async()=>{for(let n=0;n<300;n++){const data=await get('/api/skills');if(!data.busy)return data;await new Promise(r=>setTimeout(r,100));}throw Error('Inventory stayed busy');};
  await post({action:'scan'});let report=await idle();
  assert.ok(report.machines.every(m=>!m.error),JSON.stringify(report.machines.map(m=>({host:m.host,error:m.error}))));
  const source=()=>report.machines.find(m=>m.host==='local').copies.find(c=>c.name==='rename-before');
  // Reject a collision before changing the original Markdown or directory.
  await put(join(local,'.agents/skills/rename-taken/SKILL.md'),text('rename-taken'));
  let failure=await post({action:'save_file',skill:'rename-before',file_path:'SKILL.md',content:text('rename-taken'),base_digest:source().digest},false);
  assert.match(JSON.stringify(failure),/already exists/);
  assert.equal(await readFile(join(local,'.codex/skills/rename-before/SKILL.md'),'utf8'),text('rename-before'));
  failure=await post({action:'save_file',skill:'rename-before',file_path:'SKILL.md',content:text('../escape'),base_digest:source().digest},false);
  assert.match(JSON.stringify(failure),/name/i);
  // Save locally first; a subsequent distribution must remember the remote rename.
  report=await post({action:'save_file',skill:'rename-before',file_path:'SKILL.md',content:text('rename-after'),base_digest:source().digest});
  assert.equal(report.saved_skill,'rename-after');
  assert.ok(report.selected.includes('rename-after'));assert.ok(!report.selected.includes('rename-before'));
  assert.equal(await exists(join(local,'.codex/skills/rename-before')),false);
  assert.equal(await exists(join(remote,'.codex/skills/rename-before')),true);
  server.kill();await new Promise(r=>server.once('exit',r));await start();
  report=await get('/api/skills');
  // Detect unseen remote edits, preserve them, then allow an explicit reviewed retry.
  await put(join(remote,'.codex/skills/rename-before/SKILL.md'),text('rename-before')+'Remote edit\n');
  await post({action:'distribute',revision:report.revision,skill:'rename-after',selected:report.selected,choices:report.choices});
  report=await idle();
  assert.match(report.machines.find(m=>m.host==='fixture-remote').error,/changed since/);
  assert.match(await readFile(join(remote,'.codex/skills/rename-before/SKILL.md'),'utf8'),/Remote edit/);
  await post({action:'distribute',revision:report.revision,skill:'rename-after',selected:report.selected,choices:report.choices});
  report=await idle();
  assert.ok(report.machines.every(m=>!m.error),JSON.stringify(report.machines.map(m=>({host:m.host,error:m.error}))));
  for(const home of [local,remote])for(const agent of ['.codex','.agents','.claude','.hey-boss']){
    assert.equal(await exists(join(home,agent,'skills/rename-before')),false);
    assert.equal(await readFile(join(home,agent,'skills/rename-after/references/notes.md'),'utf8'),'# Reference\n');
    assert.equal(await readFile(join(home,agent,'skills/rename-after/SKILL.md'),'utf8'),text('rename-after'));
  }
  const {chromium}=await import(process.env.PLAYWRIGHT_MODULE||'playwright');
  browser=await chromium.launch({channel:'chrome',headless:true});
  const page=await browser.newPage({viewport:{width:1440,height:1000}}), errors=[];
  page.on('pageerror',error=>errors.push(error.message));
  await page.goto(origin+'/skills#skill=global%3Arename-after');
  await page.locator('[data-toggle-editor]').click();
  const edit=async content=>{
    const cm=page.locator('.cm-content');
    await cm.waitFor({state:'visible'});await cm.fill(content);
  };
  await edit(text('rename-final'));
  if(process.env.SKILLS_SCREENSHOTS){await mkdir(process.env.SKILLS_SCREENSHOTS,{recursive:true});await page.screenshot({path:join(process.env.SKILLS_SCREENSHOTS,'editing.png'),fullPage:true});}
  await page.locator('[data-save-markdown="distribute"]').click();
  await page.waitForFunction(()=>document.querySelector('.skill-detail-heading h2')?.textContent==='rename-final');
  assert.equal(new URLSearchParams(new URL(page.url()).hash.slice(1)).get('skill'),'global:rename-final');
  report=await idle();
  assert.ok(report.machines.every(m=>!m.error),JSON.stringify(report.machines.map(m=>m.error)));
  assert.equal(await exists(join(remote,'.codex/skills/rename-after')),false);
  assert.equal(await readFile(join(remote,'.codex/skills/rename-final/SKILL.md'),'utf8'),text('rename-final'));
  // A rejected save must preserve the editable draft.
  await page.reload();await page.locator('[data-toggle-editor]').click();await edit(text('rename-taken'));
  await page.locator('[data-save-markdown="local"]').click();
  await page.waitForFunction(()=>!document.querySelector('#skills-error').hidden);
  assert.equal(await page.locator('[data-save-markdown="local"]').count(),1);
  assert.match(await page.locator('.cm-content, #skill-markdown-textarea:visible').first().textContent().catch(()=>''),/rename-taken/);
  await page.setViewportSize({width:390,height:844});
  const card=await page.locator('.skill-markdown-editor-card').boundingBox();
  for(const button of await page.locator('.skill-markdown-editor-actions button').all()){
    const box=await button.boundingBox();assert.ok(box.x>=card.x&&box.x+box.width<=card.x+card.width,'Editor actions fit on narrow screens');
  }
  if(process.env.SKILLS_SCREENSHOTS)await page.screenshot({path:join(process.env.SKILLS_SCREENSHOTS,'rejected-mobile.png'),fullPage:true});
  assert.deepEqual(errors,[]);
  console.log('Skill rename: collisions, path safety, local save, later sync, real browser rename, URL, and retained drafts passed');
}finally{
  await browser?.close();
  if(server&&server.exitCode===null){server.kill();await new Promise(r=>server.once('exit',r));}
  await rm(root,{recursive:true,force:true});
}
