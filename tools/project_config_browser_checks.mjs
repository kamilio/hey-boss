// Real SQLite/API/CLI round trip with an isolated web service and Chrome.
import assert from 'node:assert/strict';
import {mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync} from 'node:fs';
import {resolve, join} from 'node:path';
import {spawn, spawnSync} from 'node:child_process';
const {chromium} = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const binary = resolve(process.argv[2]);
const root = mkdtempSync(resolve('.config-browser-'));
const project = 'named:Configuration browser verification';
const env = {...process.env, HEY_BOSS_AGENT_ID:'human:verification', HEY_BOSS_ISSUE_DB:join(root,'issues.db'), HEY_BOSS_FLEET_STATE:root, HEY_BOSS_INBOX_SOCKET:join(root,'inbox.sock'), TMPDIR:root};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_STATE_DIR']) delete env[key];
const run = args => new Promise((yes,no) => {
  const child = spawn(binary,args,{env,cwd:root,stdio:['ignore','pipe','pipe']});
  let output='',errors=''; child.stdout.on('data',v=>output+=v); child.stderr.on('data',v=>errors+=v);
  child.on('error',no); child.on('exit',code => code === 0 ? yes(JSON.parse(output)) : no(Error(errors || output)));
});
let web, browser;
const checks=[];
const check=(condition,name)=>{assert.ok(condition,name);checks.push(name);};
try {
  assert.equal(spawnSync('git',['init','-q'],{cwd:root}).status,0);
  // Own the database before CLI writes so no detached companion outlives this fixture.
  web=spawn(binary,['issue','web','--port','0','--no-discovery','--project',project,'--json'],{env,cwd:root,stdio:['ignore','pipe','pipe']});
  const info=await new Promise((yes,no)=>{
    let text='',errors=''; const timer=setTimeout(()=>no(Error('Web startup timed out: '+errors)),60000);
    web.stderr.on('data',v=>errors+=v); web.on('error',no);
    web.stdout.on('data',v=>{text+=v;if(text.includes('\n')){clearTimeout(timer);try{yes(JSON.parse(text.split('\n')[0]));}catch(e){no(e);}}});
  });
  await run(['project','init','--project',project,'--prs','false','--worktree','false','--yes','--json']);
  await run(['config','sync','--json']);
  browser=await chromium.launch({channel:'chrome',headless:true});
  const page=await browser.newPage({viewport:{width:1280,height:900}}), errors=[];
  page.on('pageerror',error=>errors.push(error.message));
  await page.goto(info.url);
  await page.locator('#project-settings-trigger').click();
  await page.waitForFunction(()=>!document.querySelector('#project-prs').disabled);
  await page.locator('#project-prs').check();
  await page.getByRole('button',{name:'Save',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('#project-settings-dialog').open);
  let history=await run(['config','log','--json']);
  check(history.revisions[0].actor.source==='web interface','Web save has editor attribution');
  const first=history.revisions[0].id;
  await run(['config','sync','--json']);
  const yaml=join(root,'.hey-boss/config.yaml');
  check(readFileSync(yaml,'utf8').includes('prs_enabled: true'),'Web save materializes to YAML');
  writeFileSync(yaml,readFileSync(yaml,'utf8').replace('worktree_enabled: false','worktree_enabled: true'));
  await run(['config','sync','--json']);
  history=await run(['config','log','--json']);
  check(history.revisions[0].actor.source==='filesystem','File save has honest attribution');
  await page.locator('#project-settings-trigger').click();
  await page.getByRole('tab',{name:'Activity',exact:true}).click();
  await page.locator('.config-history-entry').nth(1).waitFor();
  await page.locator('.config-history-entry').nth(1).getByRole('button',{name:'View change'}).click();
  await page.getByRole('button',{name:'Undo this change…',exact:true}).click();
  check((await page.locator('#config-history-detail').innerText()).includes('Pull requests'),'Undo preview names the affected field');
  mkdirSync(resolve('.config-qa'),{recursive:true});
  await page.screenshot({path:resolve('.config-qa/undo-preview.png')});
  await page.getByRole('button',{name:'Undo these changes',exact:true}).click();
  await page.getByText('Undid a change',{exact:true}).waitFor();
  const settings=await run(['issue','settings','show','--json']);
  check(!settings.prs_enabled && settings.worktree_enabled,'Browser undo preserves unrelated file edit');
  history=await run(['config','log','--json']);
  check(history.revisions[0].action==='undo' && history.revisions[0].target===first,'Undo is a new linked revision');
  await page.locator('.config-history-entry').first().getByRole('button',{name:'View change'}).click();
  await page.getByRole('button',{name:'Undo this change…',exact:true}).click();
  // Another editor changes settings after the preview; confirmation must show an error.
  await run(['issue','settings','set','--no-worktree','--json']);
  await page.getByRole('button',{name:'Undo these changes',exact:true}).click();
  await page.getByText('Settings changed since the preview; refresh to review the current changes',{exact:true}).waitFor();
  check(!(await run(['issue','settings','show','--json'])).prs_enabled,'Stale undo preserves current data');
  await page.getByRole('button',{name:'Close change',exact:true}).click();
  await page.getByRole('button',{name:'Refresh',exact:true}).click();
  await page.locator('.config-history-entry').first().waitFor();
  await page.locator('.config-history-entry').last().getByRole('button',{name:'View change'}).click();
  await page.getByRole('button',{name:'Restore this version…',exact:true}).click();
  await page.getByRole('button',{name:'Restore these settings',exact:true}).click();
  await page.getByText('Restored a version',{exact:true}).waitFor();
  const restored=await run(['issue','settings','show','--json']);
  check(restored.prs_enabled && !restored.worktree_enabled,'Browser restore applies the complete snapshot');
  mkdirSync(resolve('.config-qa'),{recursive:true});
  for (const width of [1280,768,390,320]) for (const colorScheme of ['light','dark']) {
    await page.setViewportSize({width,height:900}); await page.emulateMedia({colorScheme});
    check(await page.locator('#project-settings-dialog').evaluate(el=>el.scrollWidth<=el.clientWidth+1),'Activity fits '+width+' '+colorScheme);
    check(await page.locator('#project-settings-close').isVisible(),'Close remains accessible');
    await page.screenshot({path:resolve('.config-qa',`activity-${width}-${colorScheme}.png`)});
  }
  check(errors.length===0,'No browser exceptions: '+errors.join('; '));
  console.log(JSON.stringify({status:'passed',checks:checks.length,details:checks},null,2));
} finally {
  if(browser)await browser.close();
  if(web){await new Promise(resolve=>{const timer=setTimeout(()=>web.kill('SIGKILL'),2000);web.once('exit',()=>{clearTimeout(timer);resolve();});web.kill('SIGTERM');});}
  rmSync(root,{recursive:true,force:true});
}
