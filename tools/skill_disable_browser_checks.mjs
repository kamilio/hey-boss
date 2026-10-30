// Real HTTP, fleet protocol, and browser checks using disposable homes.
import assert from 'node:assert/strict';
import {mkdtemp,mkdir,writeFile,readFile,rm,chmod,access,stat} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawn} from 'node:child_process';
const binary=resolve(process.env.HEY_BOSS_TEST_BINARY||'target/debug/hey-boss');
const root=await mkdtemp(join(tmpdir(),'skill-disable-'));
const local=join(root,'local'),remote=join(root,'remote');
const put=async(p,s)=>{await mkdir(resolve(p,'..'),{recursive:true});await writeFile(p,s);};
const exists=p=>access(p).then(()=>true,()=>false);
const text='---\nname: example\ndescription: Fixture\n---\n';
const roots=['.codex','.agents','.claude','.hey-boss'];
let server,browser,log='',origin,csrf;
const env={...process.env,HOME:local,PATH:`${join(root,'bin')}:${resolve(binary,'..')}:${process.env.PATH}`,HEY_BOSS_STATE_DIR:join(root,'state')};
for(const key of ['HEY_BOSS_FLEET_CONFIG','HEY_BOSS_FLEET_STATE','HEY_BOSS_FLEET_DESIRED','HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_DB'])delete env[key];
const cli=(args)=>new Promise(done=>{const child=spawn(binary,args,{cwd:local,env});let out='';child.stdout.on('data',c=>out+=c);child.stderr.on('data',c=>out+=c);child.on('exit',code=>done({code,out}));});
try{
  for(const home of [local,remote])for(const agent of roots){
    await put(join(home,agent,'skills/example/SKILL.md'),text);
    await put(join(home,agent,'skills/example/.hidden'),Buffer.from([0,128,255]));
    await put(join(home,agent,'skills/example/script'),'#!/bin/sh\nexit 0\n');
    await chmod(join(home,agent,'skills/example/script'),0o751);
  }
  await put(join(local,'.hey-boss/config.json'),JSON.stringify({ssh_hosts:['fixture-remote']}));
  await put(join(root,'bin/ssh'),`#!/usr/bin/env node\nconst {spawnSync}=require('node:child_process'),{existsSync}=require('node:fs');if(existsSync(${JSON.stringify(join(root,'offline'))})){console.error('Fixture offline');process.exit(1);}const command=process.argv.at(-1);const r=command.includes('skill apply-enabled')?spawnSync(${JSON.stringify(binary)},['skill','apply-enabled'],{stdio:'inherit',env:{...process.env,HOME:${JSON.stringify(remote)}}}):spawnSync('python3',[${JSON.stringify(resolve('src/skill/transport.py'))}],{stdio:'inherit',env:{...process.env,HOME:${JSON.stringify(remote)}}});process.exit(r.status??1);\n`);
  await chmod(join(root,'bin/ssh'),0o755);
  const start=async()=>{
    log='';origin='';server=spawn(binary,['issue','web','--port','0','--no-discovery','--json'],{cwd:local,env});
    server.stdout.on('data',c=>log+=c);server.stderr.on('data',c=>log+=c);
    for(let n=0;n<300&&!origin;n++){for(const line of log.split('\n'))try{origin=JSON.parse(line).url||origin;}catch{}if(server.exitCode!==null)throw Error(log);if(!origin)await new Promise(r=>setTimeout(r,100));}
    assert.ok(origin,log);origin=origin.replace(/\/$/,'');csrf=(await (await fetch(origin+'/api/bootstrap')).json()).csrf;
  };
  await start();
  const get=async()=>(await fetch(origin+'/api/skills')).json();
  const post=async(body,ok=true)=>{const r=await fetch(origin+'/api/skills',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':csrf},body:JSON.stringify(body)});const value=await r.json();assert.equal(r.ok,ok,JSON.stringify(value));return value;};
  const idle=async()=>{for(let n=0;n<300;n++){const r=await get();if(!r.busy)return r;await new Promise(r=>setTimeout(r,100));}throw Error('Still busy');};
  await post({action:'scan'});let report=await idle();assert.ok(report.machines.every(m=>!m.error));
  for(const path of ['/','/skills','/jobs','/secrets']){
    const html=await (await fetch(origin+path)).text();
    assert.match(html.split('id="navigation-more-links"')[1].split('</nav>')[0],/id="nav-skills"/);
    assert.doesNotMatch(html.split('aria-label="Main navigation"')[1].split('</nav>')[0],/id="nav-skills"/);
  }
  const {chromium}=await import(process.env.PLAYWRIGHT_MODULE||'playwright');
  browser=await chromium.launch({channel:'chrome',headless:true});
  const page=await browser.newPage({viewport:{width:1440,height:1000}}),errors=[];
  page.on('pageerror',e=>errors.push(e.message));
  await page.goto(origin+'/skills#skill=global%3Aexample');
  await page.locator('#navigation-more').click();await page.locator('#nav-skills').waitFor({state:'visible'});
  await page.locator('#navigation-more').click();
  await put(join(root,'offline'),'');
  await page.locator('[data-disable-skill="example"]').click();
  report=await idle();assert.ok(report.disabled.includes('example'));assert.match(report.message,/needs attention/);
  assert.match(report.machines.find(m=>m.host==='fixture-remote').error,/offline/);
  for(const agent of roots)assert.equal(await exists(join(local,agent,'skills/example')),false);
  assert.equal(await exists(join(remote,'.codex/skills/example')),true);
  await page.locator('[data-disable-skill="example"]').waitFor({state:'visible'});
  await rm(join(root,'offline'));await page.locator('[data-disable-skill="example"]').click();
  report=await idle();assert.ok(report.machines.every(m=>!m.error));
  for(const home of [local,remote])for(const agent of roots)assert.equal(await exists(join(home,agent,'skills/example')),false);
  await post({action:'save_file',skill:'example',content:text},false);
  await post({action:'distribute',revision:report.revision,selected:['example']},false);
  assert.equal((await cli(['skill','sync','example'])).code,0);
  assert.equal(await exists(join(local,'.codex/skills/example')),false);
  server.kill();await new Promise(r=>server.once('exit',r));await start();
  await page.goto(origin+'/skills#skill=global%3Aexample');
  await page.locator('[data-filter="disabled"]').click();
  await page.locator('[data-enable-skill="example"]').waitFor({state:'visible'});
  if(process.env.SKILLS_SCREENSHOTS){await mkdir(process.env.SKILLS_SCREENSHOTS,{recursive:true});await page.screenshot({path:join(process.env.SKILLS_SCREENSHOTS,'disabled.png'),fullPage:true});}
  await page.setViewportSize({width:390,height:844});
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Mobile must not overflow');
  if(process.env.SKILLS_SCREENSHOTS)await page.screenshot({path:join(process.env.SKILLS_SCREENSHOTS,'disabled-mobile.png'),fullPage:true});
  await page.locator('[data-enable-skill="example"]').click();report=await idle();
  assert.ok(!report.disabled.includes('example'));assert.ok(report.machines.every(m=>!m.error));
  for(const home of [local,remote])for(const agent of roots){
    const dir=join(home,agent,'skills/example');
    assert.equal(await readFile(join(dir,'SKILL.md'),'utf8'),text);
    assert.deepEqual(await readFile(join(dir,'.hidden')),Buffer.from([0,128,255]));
    assert.equal((await stat(join(dir,'script'))).mode&0o777,0o751);
  }
  assert.deepEqual(errors,[]);
  console.log('PASS global menu, disable/enable, complete archives, offline retry, sync protection, restart persistence, and mobile layout');
}finally{
  await browser?.close();server?.kill();if(server&&server.exitCode===null)await new Promise(r=>server.once('exit',r));await rm(root,{recursive:true,force:true});
}
