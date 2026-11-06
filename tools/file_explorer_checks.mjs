import assert from 'node:assert/strict';
import {createServer} from 'node:net';
import {mkdirSync, readFileSync, writeFileSync, existsSync} from 'node:fs';
import {join,resolve} from 'node:path';

export async function checkExplorer(page, root, run, origin) {
  await page.goto(origin);
  const dialog = page.locator('.file-explorer'), find = dialog.getByRole('searchbox');
  const open = name => dialog.getByRole('button',{name,exact:true}).click();
  const edit = dialog.locator('.cm-content');
  await page.locator('#project-more').click();
  await page.locator('#project-settings-trigger').click();
  const settings = page.locator('#project-settings-dialog');
  assert.deepEqual(await settings.getByRole('tab').allTextContents(),['Configuration','Activity']);
  const expectedFiles = ['prs.md','main.md','worktree.md','checkout.md','worker.md','plan.md'];
  assert.deepEqual(await settings.locator('[data-prompt-file]').allTextContents(),expectedFiles);
  await settings.getByRole('link',{name:'prs.md',exact:true}).click();
  await edit.waitFor();
  assert.equal(await dialog.locator('h3').innerText(),'prs.md');
  assert.equal(await page.locator('#nav-prompts').getAttribute('aria-current'),'page');
  await page.locator('#project-more').click();
  await page.locator('#project-settings-trigger').click();
  await page.locator('#project-prs:enabled').waitFor();
  assert.equal(await settings.locator('#project-settings-name').innerText(), 'Configuration browser verification');
  await page.getByRole('button',{name:'Cancel',exact:true}).click();
  await page.locator('#project-more').click();
  await page.locator('#nav-prompts').click();
  await edit.waitFor();
  assert.equal(await dialog.locator('h3').innerText(),'worker.md');
  await open('handoff.md');
  const saved=await run(['issue','settings','show','--json']);
  const initial=saved.prompt_defaults.handoff;
  await edit.fill(initial+'!');
  page.once('dialog',d=>d.dismiss());
  await open('plan.md'); assert.equal(await dialog.locator('h3').innerText(),'handoff.md');
  await open('Save'); await dialog.getByText('Saved',{exact:true}).waitFor();
  assert.equal((await run(['issue','settings','show','--json'])).prompt_overrides.handoff, initial+'!');
  await page.reload(); await edit.waitFor(); assert.equal((await edit.locator('.cm-line').allTextContents()).join('\n'),initial+'!');
  page.once('dialog',d=>d.accept());
  await open('Restore default…'); await dialog.getByText('Default restored').waitFor();
  assert.equal((await run(['issue','settings','show','--json'])).prompt_overrides.handoff,undefined);
  if (process.platform==='darwin') {
    const token=await page.evaluate(async()=>(await (await fetch('/api/bootstrap')).json()).csrf);
    const refused=await page.request.post(new URL('/api/prompts/open',origin).href,{
      headers:{'x-hey-boss-csrf':token},data:{project:saved.project.id,key:'../config.yaml'}
    });
    assert.equal(refused.status(),400);
    // Exercise the real socket payload without opening a window in someone's desktop session.
    let launch;
    const socket=createServer({allowHalfOpen:true},client=>{
      let bytes='';client.on('data',v=>bytes+=v);client.on('end',()=>{
        const msg=JSON.parse(bytes); if(msg.command==='artifact_editor')launch=JSON.parse(msg.question);
        client.end(JSON.stringify({status:'ok', result:'{"ok":true,"unread":0,"tasks":[]}'}));
      });
    });
    await new Promise((ok,no)=>{socket.on('error',no);socket.listen(join(root,'inbox.sock'),ok);});
    try {
      await open('Open in Mac editor'); await dialog.getByText('Opened in Mac editor').waitFor();
      assert.ok(launch.file.startsWith(join(root,'project-configs')+'/') && launch.file.endsWith('/.hey-boss/prompts/handoff.md'));
      assert.equal(readFileSync(launch.file,'utf8'),initial);
    } finally {await new Promise(ok=>socket.close(ok));}
  }
  await find.fill('handoff');await open('handoff.md');await edit.fill(initial+'?');
  await run(['issue','settings','set','--no-prs','--json']);
  await open('Save');await dialog.locator('.fx-error').waitFor();
  assert.equal((await edit.locator('.cm-line').allTextContents()).join('\n'),initial+'?');
  assert.equal((await run(['issue','settings','show','--json'])).prompt_overrides.handoff,undefined);
  page.once('dialog',d=>d.accept()); await open('prompts'); await find.fill(''); await open('worker.md'); await edit.waitFor();
  for(const width of [1280,390]){
    await page.setViewportSize({width,height:800}); await page.emulateMedia({colorScheme:width===390?'dark':'light'});
    assert.ok(await dialog.evaluate(el=>el.scrollWidth<=el.clientWidth+1));
    await page.evaluate(()=>new Promise(ok=>requestAnimationFrame(()=>requestAnimationFrame(ok))));
    await page.screenshot({path:resolve('.config-qa','files-'+width+'.png')});
  }
  const site=join(root,'site');mkdirSync(join(site,'docs'),{recursive:true});
  writeFileSync(join(site,'index.html'),'<h1>Example</h1>');writeFileSync(join(site,'docs/note.md'),'# Note');
  await run(['pages','publish',site,'--name','example','--json']);
  await page.goto(new URL('/pages/example',origin).href);
  await page.locator('#page-files').click();await edit.waitFor();
  assert.equal(await edit.innerText(),'<h1>Example</h1>');
  await open('docs'); await open('note.md');await edit.fill('# Changed');
  await open('Save & publish');await dialog.getByText('Saved',{exact:true}).waitFor();
  await open('Move…'); await page.getByLabel('Destination path').fill('notes/moved.md');
  await page.getByRole('button',{name:'Move file',exact:true}).click();await dialog.getByText('Moved',{exact:true}).waitFor();
  const output=join(root,'export');
  await run(['pages','download','example','--output',output,'--json']);
  assert.equal(readFileSync(join(output,'notes/moved.md'),'utf8'),'# Changed');assert.ok(!existsSync(join(output,'docs/note.md')));
  page.once('dialog',d=>d.accept());await open('Delete…');await dialog.getByText('File deleted').waitFor();
  const pages=await run(['pages','list','--json']);assert.equal(pages.pages.find(p=>p.name==='example').files,1);
  await open('Close');await page.setViewportSize({width:1280,height:900});
}
