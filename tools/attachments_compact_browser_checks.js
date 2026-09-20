// Run with playwright-cli run-code --filename against an isolated issue web fixture.
// Seed issue 1 in named:Attachment Studio. Put release-notes.txt and
// release-notes-with-a-very-long-filename-that-must-wrap-on-small-mobile-screens.txt
// in output/playwright/issue63/. This check removes that issue's fixture files.
async page => {
  const checks = [], errors = [];
  page.on('pageerror', e => errors.push(e.message));
  if(await page.locator('#comment-body').count())await page.locator('#comment-body').fill('');
  await page.reload();
  const panel = page.locator('#issue-attachments');
  const editor = page.locator('#comment-form .markdown-editor');
  const choose = editor.getByRole('button', {name:'Attach files', exact:true});
  await page.locator('#comment-body').waitFor();
  await choose.waitFor({timeout:5000});
  if (await page.locator('.attachment-drop').count()) throw Error('Prominent drop zone still present');
  await page.waitForFunction(() => !document.querySelector('.attachment-status').textContent.includes('Loading'));
  while(await panel.locator('[data-remove]').count()) {
    await panel.locator('[data-remove]').first().click();await panel.getByRole('button',{name:'Remove',exact:true}).click();
    await page.waitForFunction(()=>document.querySelector('#issue-attachments').dataset.busy==='false');
  }
  await page.reload();await choose.waitFor();
  await page.waitForFunction(() => !document.querySelector('.attachment-status').textContent.includes('Loading'));
  if (await panel.locator('.attachment-name').count() === 0 && await panel.isVisible()) throw Error('Empty attachment panel still takes space');
  await panel.locator('input[type=file]').setInputFiles('output/playwright/issue63/release-notes.txt');
  await panel.getByRole('button', {name:'release-notes.txt', exact:true}).waitFor();
  checks.push('file-picker-upload');
  const drag = (type, file=true, large=false) => editor.evaluate((el, {type,file,large}) => {
    const dt=new DataTransfer();
    if(file) dt.items.add(new File([large ? new Uint8Array(10*1024*1024+1) : 'Dropped file'], large?'too-large.bin':'dropped.txt'));
    else dt.setData('text/plain','Ordinary text');
    const event=new DragEvent(type,{bubbles:true,cancelable:true,dataTransfer:dt});
    el.querySelector('textarea').dispatchEvent(event);
    return {prevented:event.defaultPrevented,highlight:el.classList.contains('attachment-drag-over')};
  }, {type,file,large});
  if ((await drag('dragover',false)).prevented) throw Error('Text drag was intercepted');
  if (!(await drag('dragenter')).highlight) throw Error('Input is not highlighted for file drops');
  await page.screenshot({path:'output/playwright/issue63/input-drag.png',fullPage:true});
  if ((await drag('dragleave')).highlight) throw Error('Highlight remains after drag leave');
  await page.locator('#comment-body').fill('Unsaved comment stays intact.');
  await drag('drop');
  await panel.getByRole('button',{name:'dropped.txt',exact:true}).waitFor();
  if (await page.locator('#comment-body').inputValue() !== 'Unsaved comment stays intact.') throw Error('Drop changed comment');
  await page.locator('#comment-body').fill('');
  checks.push('input-drop-highlight-and-text-preservation');
  const [download]=await Promise.all([page.waitForEvent('download'),panel.getByRole('button',{name:'dropped.txt',exact:true}).click()]);
  const stream=await download.createReadStream(),chunks=[];
  for await(const chunk of stream)chunks.push(...chunk);
  if(String.fromCharCode(...chunks)!=='Dropped file')throw Error('Download bytes differ');
  await page.waitForFunction(()=>document.querySelector('#issue-attachments').dataset.busy==='false');
  await panel.getByRole('button',{name:'Remove dropped.txt',exact:true}).click();
  await panel.getByRole('button',{name:'Keep file',exact:true}).click();
  await panel.getByRole('button',{name:'Remove dropped.txt',exact:true}).click();
  await panel.getByRole('button',{name:'Remove',exact:true}).click();
  await panel.getByRole('button',{name:'dropped.txt',exact:true}).waitFor({state:'detached'});
  checks.push('download-and-confirmed-removal');
  let lost=true;const ids=[];
  await page.route('**/api/action',async route=>{
    const body=route.request().postDataJSON();
    if(body.operation?.action==='attachment'&&body.operation.operation.command==='upload') {
      ids.push(body.request_id);
      if(lost){lost=false;await route.fetch();await route.fulfill({status:503,contentType:'application/json',body:JSON.stringify({ok:false,error:{message:'Synthetic lost acknowledgment'}})});return;}
    }
    await route.continue();
  });
  await drag('drop');await panel.getByRole('button',{name:'Retry',exact:true}).click();
  await panel.getByRole('button',{name:'dropped.txt',exact:true}).waitFor();
  if(ids.length!==2||ids[0]!==ids[1])throw Error('Retry changed upload identity');
  await page.unroute('**/api/action');await page.reload();await choose.waitFor();
  await panel.getByRole('button',{name:'dropped.txt',exact:true}).waitFor();
  if(await panel.getByRole('button',{name:'dropped.txt',exact:true}).count()!==1)throw Error('Retry duplicated file');
  checks.push('idempotent-upload-retry');
  await drag('drop',true,true);
  await panel.locator('.attachment-error').filter({hasText:'exceeds 10 MB'}).waitFor();
  checks.push('oversized-drop-error');
  const longName='release-notes-with-a-very-long-filename-that-must-wrap-on-small-mobile-screens.txt';
  await panel.locator('input[type=file]').setInputFiles(`output/playwright/issue63/${longName}`);
  await panel.getByRole('button',{name:longName,exact:true}).waitFor();
  for(const theme of ['light','dark']) {
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    for(const [width,height] of [[1440,1000],[768,1024],[390,844],[320,740]]) {
      await page.setViewportSize({width,height});
      if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1||[...document.querySelectorAll('.attachment-list li')].some(e=>e.scrollWidth>e.clientWidth+1)))throw Error(`Overflow at ${theme}-${width}`);
      await page.screenshot({path:`output/playwright/issue63/${theme}-${width}.png`,fullPage:true});
      checks.push(`${theme}-${width}`);
    }
  }
  let release, entered;
  const held=new Promise(resolve=>release=resolve),started=new Promise(resolve=>entered=resolve);
  await page.route('**/api/action',async route=>{
    const body=route.request().postDataJSON();
    if(body.operation?.action==='attachment'&&body.operation.operation.command==='upload'){entered();await held;}
    await route.continue();
  });
  const upload=panel.locator('input[type=file]').setInputFiles('output/playwright/issue63/release-notes.txt');
  await started;
  if(!await choose.isDisabled())throw Error('External paperclip remains enabled during upload');
  if((await drag('dragenter')).highlight)throw Error('Busy input advertises file drops');
  await drag('drop');release();await upload;
  await page.waitForFunction(()=>document.querySelector('#issue-attachments').dataset.busy==='false');
  await page.unroute('**/api/action');
  if(await choose.isDisabled())throw Error('Paperclip remains disabled after upload');
  if(await panel.getByRole('button',{name:'dropped.txt',exact:true}).count()!==1)throw Error('Busy drop uploaded another file');
  checks.push('external-paperclip-busy-state');
  await page.evaluate(async()=>{
    const boot=await(await fetch('/api/bootstrap')).json(),root=document.createElement('section');
    root.id='readonly-attachment-fixture';document.querySelector('main').append(root);
    HeyBossAttachments.mount(root,{project:'named:Attachment Studio',target:{kind:'issue',id:'1'},csrf:boot.csrf,readonly:true});
  });
  const readonly=page.locator('#readonly-attachment-fixture');
  await readonly.locator('.attachment-name').first().waitFor();
  if(await readonly.locator('[data-choose], [data-remove], input[type=file]').count())throw Error('Read-only files expose write controls');
  await readonly.evaluate(root=>root.remove());checks.push('readonly-download-only');
  if(errors.length)throw Error(errors.join('\n'));
  return {checks,scriptErrors:errors};
}
