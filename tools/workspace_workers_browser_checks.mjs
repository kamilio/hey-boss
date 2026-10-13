// Real supervisor/API/browser regression; all state is disposable and workers stay paused.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, writeFile, access, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {spawn} from 'node:child_process';

const binary=resolve(process.env.HEY_BOSS_TEST_BINARY||'target/debug/hey-boss');
const root=await mkdtemp(join(tmpdir(),'worker-workspace-'));
const children=[];
let browser,log='';
try{
  const desired=join(root,'fleet.yaml');
  await writeFile(desired,'machines:\n  local:\n    workers:\n      - id: tools\n        intent: pause\n        config:\n          name: Tools\n          concurrency: 2\n          tags: [review]\n');
  const env={...process.env,HOME:root,HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_FLEET_STATE:root,HEY_BOSS_FLEET_DESIRED:desired,HEY_BOSS_FLEET_BINARY:binary};
  delete env.HEY_BOSS_ISSUE_HOST;delete env.HEY_BOSS_ISSUE_PROJECT;
  const start=args=>{
    const child=spawn(binary,args,{cwd:root,env});children.push(child);
    child.stdout.on('data',chunk=>log+=chunk);child.stderr.on('data',chunk=>log+=chunk);
    return child;
  };
  const until=async predicate=>{for(let n=0;n<300;n++){if(await predicate())return;await new Promise(r=>setTimeout(r,100));}throw Error(log);};
  start(['fleet','supervisor']);
  await until(()=>access(join(root,'fleet.sock')).then(()=>true,()=>false));
  start(['issue','web','--port','0','--no-discovery','--project','Atlas','--json']);
  let origin;
  await until(()=>{for(const line of log.split('\n')){try{origin=JSON.parse(line).url||origin;}catch{}}return origin;});
  origin=origin.replace(/\/$/,'');
  const {chromium}=await import(process.env.PLAYWRIGHT_MODULE||'playwright');
  browser=await chromium.launch({channel:'chrome',headless:true});
  const page=await browser.newPage({viewport:{width:1440,height:1000}}), errors=[];
  page.setDefaultTimeout(15000);page.on('pageerror',e=>errors.push(e.message));
  const configuration=()=>page.request.get(origin+'/api/fleet/configuration').then(r=>r.json());
  const worker=async()=>(await configuration()).document.machines.local.workers[0];
  const open=async()=>{
    await page.locator('[data-edit-worker="tools"]').click();
    await page.locator('#worker-editor').waitFor({state:'visible'});
    await page.waitForFunction(()=>!document.getElementById('worker-name').disabled);
  };
  const review=async()=>{
    await page.locator('#worker-editor-preview').click();
    await page.waitForFunction(()=>!document.getElementById('worker-editor-save').disabled);
  };
  const save=async()=>{await page.locator('#worker-editor-save').click();await page.locator('#worker-editor').waitFor({state:'hidden'});};
  await page.goto(origin+'/workers#view=configuration');
  await page.locator('.worker-scope-heading').first().click();await open();
  const workspace=page.locator('#worker-workspace');
  assert.equal(await workspace.count(),1,'Worker settings exposes a Workspace selector');
  assert.equal(await workspace.inputValue(),'false','Unset worktree setting uses existing checkout');
  await workspace.selectOption('true');await review();
  assert.match(await page.locator('#worker-editor-changes').innerText(),/Workspace: Existing checkout → Dedicated worktree/);
  assert.notEqual((await worker()).config.worktree_enabled,true,'Review does not save');
  await workspace.selectOption('false');
  assert.equal(await page.locator('#worker-editor-save').isDisabled(),true,'Editing invalidates preview');
  await workspace.selectOption('true');
  await page.locator('#worker-editor-cancel').click();
  await page.locator('#fleet-confirm-dialog').waitFor({state:'visible'});
  await page.locator('#fleet-confirm-cancel').click();
  assert.equal(await workspace.inputValue(),'true','Cancel discard preserves draft');
  await review();
  for(const theme of ['light','dark'])for(const width of [1440,768,390,320]){
    await page.emulateMedia({colorScheme:theme});await page.setViewportSize({width,height:width<500?844:1000});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth&&[...document.querySelectorAll('dialog[open]')].every(d=>d.getBoundingClientRect().left>=0&&d.getBoundingClientRect().right<=innerWidth)),true,`${theme} ${width}px fits`);
    if(process.env.WORKSPACE_SCREENSHOTS){await mkdir(process.env.WORKSPACE_SCREENSHOTS,{recursive:true});await page.screenshot({path:join(process.env.WORKSPACE_SCREENSHOTS,`${theme}-${width}.png`)});}
  }
  await save();
  let saved=await worker();
  assert.equal(saved.config.worktree_enabled,true);assert.equal(saved.intent,'pause');assert.equal(saved.config.concurrency,2);assert.deepEqual(saved.config.tags,['review']);
  await page.locator('.worker-record-summary').click();
  await page.waitForFunction(()=>document.querySelector('.worker-facts')?.innerText.includes('Dedicated worktree'));
  assert.match(await page.locator('.worker-facts').innerText(),/Workspace\s+Dedicated worktree/);
  await open();assert.equal(await workspace.inputValue(),'true');
  await workspace.selectOption('false');await review();
  assert.match(await page.locator('#worker-editor-changes').innerText(),/Workspace: Dedicated worktree → Existing checkout/);
  await save();assert.equal((await worker()).config.worktree_enabled,false);
  await open();assert.equal(await workspace.inputValue(),'false');
  await page.locator('#worker-editor-preview').click();
  await page.waitForFunction(()=>document.getElementById('worker-editor-status').textContent==='No changes to save.');
  assert.equal(await page.locator('#worker-editor-save').isDisabled(),true);
  await workspace.selectOption('true');await page.locator('#worker-editor-cancel').click();await page.locator('#fleet-confirm-submit').click();
  await page.locator('#worker-editor').waitFor({state:'hidden'});
  assert.equal((await worker()).config.worktree_enabled,false,'Discard does not save');
  assert.deepEqual(errors,[]);console.log('Workspace browser checks passed: default, review, enable/disable persistence, reopen, preserved settings, unsaved changes, mobile and desktop.');
}finally{
  await browser?.close();
  await Promise.all(children.map(child=>new Promise(resolve=>{if(child.exitCode!==null)return resolve();child.once('exit',resolve);child.kill('SIGTERM');})));
  await rm(root,{recursive:true,force:true});
}
