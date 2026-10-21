// Real Jobs API and browser, with disposable project data and no running agents.
import assert from 'node:assert/strict';
import {mkdtemp, readFile, mkdir, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {spawn, spawnSync} from 'node:child_process';
const root=await mkdtemp(join(tmpdir(),'job-execution-'));
const binary=resolve(process.env.HEY_BOSS_TEST_BINARY||'target/debug/hey-boss');
const env={...process.env,HOME:root,HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:join(root,'absent.sock'),HEY_BOSS_AGENT_ID:'human:test'};
delete env.HEY_BOSS_ISSUE_HOST;delete env.HEY_BOSS_ISSUE_PROJECT;
const cli=args=>{const r=spawnSync(binary,args,{cwd:root,env,encoding:'utf8'});assert.equal(r.status,0,r.stderr+r.stdout);return JSON.parse(r.stdout);};
let child,browser,log='';
try{
  for(const project of ['Execution','Other'])cli(['project','init','--project','named:'+project,'--yes','--prs','false','--worktree','false','--json']);
  cli(['issue','settings','set','--project','named:Execution','--worker-model','project-model','--worker-reasoning','high','--worker-permissions','yolo','--json']);
  child=spawn(binary,['issue','web','--project','named:Execution','--port','0','--no-discovery','--json'],{cwd:root,env});
  child.stdout.on('data',chunk=>log+=chunk);child.stderr.on('data',chunk=>log+=chunk);
  let origin;
  for(let n=0;n<200&&!origin;n++){for(const line of log.split('\n')){try{origin=JSON.parse(line).url||origin;}catch{}}if(!origin)await new Promise(r=>setTimeout(r,100));}
  assert.ok(origin,log);
  const {chromium}=await import(process.env.PLAYWRIGHT_MODULE||'playwright');
  browser=await chromium.launch({channel:'chrome',headless:true});
  const page=await browser.newPage({viewport:{width:1440,height:1000}}),errors=[];
  page.on('pageerror',e=>errors.push(e.message));page.setDefaultTimeout(15000);
  await page.route('**/api/jobs/runtime',r=>r.fulfill({json:{ok:true,machine:'fixture',catalog:{models:[]},machines:[]}}));
  await page.goto(new URL('/jobs#project=named%3AExecution',origin).href);
  await page.locator('#job-new').click();
  await page.waitForFunction(()=>document.querySelector('#job-execution-settings')?.textContent.includes('YOLO'));
  assert.equal(await page.locator('#job-model').inputValue(),'');
  assert.equal(await page.locator('#job-model').getAttribute('placeholder'),'Project · project-model');
  assert.equal(await page.locator('#job-model').evaluate(el=>el.required),false);
  assert.match(await page.locator('#job-execution-settings').innerText(),/high/);
  await page.locator('#job-project').selectOption('named:Other');
  await page.waitForFunction(()=>document.querySelector('#job-execution-settings')?.textContent.includes('Auto'));
  assert.equal(await page.locator('#job-model').getAttribute('placeholder'),'Use project setting');
  await page.locator('#job-project').selectOption('named:Execution');
  await page.waitForFunction(()=>document.querySelector('#job-execution-settings')?.textContent.includes('YOLO'));
  await page.locator('#job-harness').selectOption('claude');
  assert.equal(await page.locator('#job-model').evaluate(el=>el.required),true);
  assert.equal(await page.locator('#job-execution-settings').isHidden(),true);
  await page.locator('#job-harness').selectOption('codex');
  await page.locator('#job-name').fill('Execution settings check');
  await page.locator('#job-markdown').fill(await readFile('src/jobs/fixtures/original.md','utf8'));
  await page.locator('#job-enabled').uncheck();
  await page.getByRole('button',{name:'Create job',exact:true}).click();
  await page.locator('#job-run').waitFor();
  const id=new URLSearchParams(new URL(page.url()).hash.slice(1)).get('job');
  const saved=()=>cli(['job','--project','named:Execution','--json','view',id]).job;
  assert.equal(saved().snapshot.definition.model,'');
  await page.reload();await page.locator('#job-model').waitFor();
  assert.equal(await page.locator('#job-model').inputValue(),'');
  await page.locator('#job-model').fill('custom-model');
  await page.getByRole('button',{name:'Save changes',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('.job-panel-subtitle')?.textContent.includes('Revision 2'));
  assert.equal(saved().snapshot.definition.model,'custom-model');
  await page.locator('#job-model-inherit').click();
  assert.equal(await page.locator('#job-model').inputValue(),'');
  await page.getByRole('button',{name:'Save changes',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('.job-panel-subtitle')?.textContent.includes('Revision 3'));
  assert.equal(saved().snapshot.definition.model,'');
  for(const theme of ['light','dark'])for(const width of [1440,768,390,320]){
    await page.emulateMedia({colorScheme:theme});await page.setViewportSize({width,height:1000});
    await page.locator('#job-model').scrollIntoViewIfNeeded();
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth&&document.querySelector('#job-dialog').scrollWidth<=document.querySelector('#job-dialog').clientWidth+1),theme+' '+width);
    if(process.env.EXECUTION_SCREENSHOTS){await mkdir(process.env.EXECUTION_SCREENSHOTS,{recursive:true});await page.locator('.job-section').filter({has:page.locator('#job-model')}).screenshot({path:join(process.env.EXECUTION_SCREENSHOTS,'job-'+theme+'-'+width+'.png')});}
  }
  cli(['issue','settings','set','--project','named:Execution','--worker-permissions','','--worker-reasoning','','--worker-model','','--json']);
  await page.reload();await page.waitForFunction(()=>document.querySelector('#job-execution-settings')?.textContent.includes('Machine default'));
  assert.equal(await page.locator('#job-model').inputValue(),'');
  assert.deepEqual(errors,[]);
  console.log('Scheduled job execution UI passed: project defaults, project switching, provider scope, blank/custom/reset persistence, live settings, responsive light/dark layout.');
}finally{
  await browser?.close();
  if(child&&child.exitCode===null)await new Promise(resolve=>{child.once('exit',resolve);child.kill('SIGKILL');});
  await rm(root,{recursive:true,force:true});
}
