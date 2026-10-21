// Real project settings API and browser; all state and processes are disposable.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {spawn, spawnSync} from 'node:child_process';
const root = await mkdtemp(join(tmpdir(), 'project-execution-'));
const binary = resolve(process.env.HEY_BOSS_TEST_BINARY || 'target/debug/hey-boss');
const env = {...process.env, HOME:root, HEY_BOSS_ISSUE_DB:join(root,'issues.db'), HEY_BOSS_FLEET_STATE:root, HEY_BOSS_INBOX_SOCKET:join(root,'absent.sock')};
delete env.HEY_BOSS_ISSUE_HOST; delete env.HEY_BOSS_ISSUE_PROJECT; delete env.HEY_BOSS_AGENT_ID;
const cli = args => {
  const result = spawnSync(binary,args,{cwd:root,env,encoding:'utf8'});
  assert.equal(result.status,0,result.stderr+result.stdout);
  return JSON.parse(result.stdout);
};
const settings = () => cli(['issue','settings','show','--project','named:Execution','--json']);
let child, browser, log = '';
try {
  cli(['project','init','--project','named:Execution','--yes','--prs','false','--worktree','false','--json']);
  cli(['project','init','--project','named:Other','--yes','--prs','false','--worktree','false','--json']);
  child = spawn(binary,['issue','web','--project','named:Execution','--port','0','--no-discovery','--json'],{cwd:root,env});
  child.stdout.on('data',chunk=>log+=chunk); child.stderr.on('data',chunk=>log+=chunk);
  let origin;
  for(let n=0;n<200&&!origin;n++){
    for(const line of log.split('\n')){try{origin=JSON.parse(line).url||origin;}catch{}}
    if(!origin)await new Promise(r=>setTimeout(r,100));
  }
  assert.ok(origin,log);
  const {chromium}=await import(process.env.PLAYWRIGHT_MODULE||'playwright');
  browser=await chromium.launch({channel:'chrome',headless:true});
  const page=await browser.newPage({viewport:{width:1440,height:1000}});
  const errors=[]; page.on('pageerror',e=>errors.push(e.message)); page.setDefaultTimeout(15000);
  await page.goto(origin);
  const open=async()=>{
    await page.locator('#project-settings-trigger').click();
    await page.waitForFunction(()=>!document.querySelector('#project-worker-model').disabled);
  };
  const save=async()=>{
    await page.locator('#project-settings-form button[type=submit]').click();
    await page.locator('#project-settings-dialog').waitFor({state:'hidden'});
  };
  await open();
  assert.equal(await page.locator('#project-worker-model').inputValue(),'');
  assert.equal(await page.locator('#project-worker-reasoning').inputValue(),'');
  assert.equal(await page.locator('#project-worker-permissions').inputValue(),'auto');
  await page.locator('#project-worker-model').fill('gpt-6-astra');
  await page.locator('#project-worker-reasoning').selectOption('high');
  await page.locator('#project-worker-permissions').selectOption('yolo');
  assert.match(await page.locator('#project-worker-permissions-help').innerText(),/No sandbox or approval requests/);
  for(const theme of ['light','dark'])for(const width of [1440,768,390,320]){
    await page.emulateMedia({colorScheme:theme});
    await page.setViewportSize({width,height:width<500?900:1000});
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth&&document.querySelector('#settings-panel-configuration').scrollWidth<=document.querySelector('#settings-panel-configuration').clientWidth+1),theme+' '+width);
    if(process.env.EXECUTION_SCREENSHOTS){
      await mkdir(process.env.EXECUTION_SCREENSHOTS,{recursive:true});
      await page.screenshot({path:join(process.env.EXECUTION_SCREENSHOTS,theme+'-'+width+'.png')});
    }
  }
  await save();
  let saved=settings();
  assert.equal(saved.worker_model,'gpt-6-astra'); assert.equal(saved.worker_reasoning,'high'); assert.equal(saved.worker_permissions,'yolo');
  const other=cli(['issue','settings','show','--project','named:Other','--json']);
  assert.equal(other.worker_permissions,'auto'); assert.equal(other.worker_model,'');
  await open();
  assert.equal(await page.locator('#project-worker-model').inputValue(),'gpt-6-astra');
  await page.locator('#project-worker-model-inherit').click();
  await page.locator('#project-worker-reasoning').selectOption('');
  await page.locator('#project-worker-permissions').selectOption('');
  await save();
  saved=settings();
  assert.equal(saved.worker_model,''); assert.equal(saved.worker_reasoning,''); assert.equal(saved.worker_permissions,'');
  await open();
  assert.equal(await page.locator('#project-worker-permissions').inputValue(),'');
  await page.locator('#project-worker-permissions').selectOption('auto');
  await page.locator('#project-settings-cancel').click();
  assert.equal(settings().worker_permissions,'','Cancel must not save');
  await open();
  await page.locator('#project-worker-permissions').selectOption('auto');
  cli(['issue','settings','set','--project','named:Execution','--worker-model','external-model','--json']);
  await page.locator('#project-settings-form button[type=submit]').click();
  await page.locator('#project-settings-error').waitFor({state:'visible'});
  assert.match(await page.locator('#project-settings-error').innerText(),/changed elsewhere/i);
  assert.equal(settings().worker_model,'external-model','A stale dialog cannot overwrite a newer edit');
  await page.locator('#project-settings-cancel').click();
  await open(); await page.locator('#project-worker-permissions').selectOption('auto'); await save();
  assert.equal(settings().worker_permissions,'auto');
  assert.deepEqual(errors,[]);
  console.log('Project execution browser checks passed: defaults, project isolation, save/reopen, all inherited values, cancel, stale-save conflict, light/dark and responsive layout.');
} finally {
  await browser?.close();
  if(child&&child.exitCode===null)await new Promise(resolve=>{child.once('exit',resolve);child.kill('SIGKILL');});
  await rm(root,{recursive:true,force:true});
}
