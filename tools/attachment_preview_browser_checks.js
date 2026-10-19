// Run against tools/serve_image_preview_fixture.mjs with playwright-cli run-code.
async page => {
  const base = new URL(page.url()).origin;
  if (base !== 'http://127.0.0.1:59661') throw Error('Use the isolated native fixture');
  const checks = [], errors = [], downloads = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  page.on('download', download => downloads.push(download));
  await page.goto(base);
  await page.waitForFunction(() => model.csrf && model.project);
  const project = 'named:Image Preview QA';
  const fixture = await page.evaluate(async project => {
    const action = operation => api(operation, project);
    const created = await action({action:'create', title:'Review the release screenshots', body:'Open an image to inspect it. Download is available separately.', labels:[]});
    const target = {kind:'issue', id:String(created.issue.number)};
    const canvas = document.createElement('canvas'); canvas.width = 1800; canvas.height = 1100;
    const ctx = canvas.getContext('2d');
    ctx.fillStyle = '#182840'; ctx.fillRect(0, 0, 1800, 1100);
    ctx.fillStyle = '#e6edf8'; ctx.font = '56px sans-serif'; ctx.fillText('Release overview', 100, 140);
    ctx.fillStyle = '#91a7c4'; ctx.font = '28px sans-serif'; ctx.fillText('A closer look at the details', 100, 200);
    for (let i = 0; i < 3; i++) {
      ctx.fillStyle = ['#63d6b4','#7ea8f8','#b2a0ef'][i]; ctx.fillRect(100, 300 + i * 180, 1100 - i * 230, 100);
      ctx.fillStyle = '#e6edf8'; ctx.fillText(['Ready to ship','In review','Next up'][i], 1250, 360 + i * 180);
    }
    ctx.fillStyle = '#91a7c4'; ctx.fillText('1800 × 1100 · Original image', 100, 1000);
    const files = [
      {name:'release-overview.png',data:canvas.toDataURL().split(',')[1]},
      {name:'photo.JPG',data:canvas.toDataURL('image/jpeg').split(',')[1]},
      {name:'image.webp',data:canvas.toDataURL('image/webp').split(',')[1]},
      {name:'pixel.gif',data:'R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7'},
      {name:'release-notes.txt',data:btoa('Original download bytes\n')},
      {name:'broken.png',data:btoa('Not an image')},
      {name:'corrupt.png',data:btoa(String.fromCharCode(137,80,78,71,13,10,26,10))},
      {name:'portrait-with-a-very-long-filename-for-narrow-phone-screens.svg', data:btoa('<svg xmlns="http://www.w3.org/2000/svg" width="600" height="1800"><rect width="600" height="1800" fill="#e8ddf9"/><circle cx="300" cy="300" r="160" fill="#8064b5"/><text x="70" y="650" font-size="48" fill="#382451">Portrait image</text></svg>')}
    ];
    for (const file of files) await action({action:'attachment',operation:{command:'upload',target,...file}});
    return {number:created.issue.number, png:files[0].data};
  }, project);
  const route = base + '/#project=' + encodeURIComponent(project) + '&issue=' + fixture.number;
  await page.goto(route);
  const opener = page.getByRole('button',{name:'release-overview.png',exact:true});
  await opener.waitFor();
  const dialog = page.locator('.attachment-viewer');
  const ready = () => page.waitForFunction(() => document.querySelector('.attachment-viewer img')?.naturalWidth === 1800);
  await opener.click();
  await dialog.waitFor({timeout:3000});
  await ready();
  check(downloads.length === 0, 'Clicking an image opens a viewer without downloading');
  check(await dialog.getAttribute('aria-label') === 'release-overview.png', 'Viewer has an accessible name');
  for (const colorScheme of ['light','dark']) for (const width of [1440,768,390,320]) {
    await page.emulateMedia({colorScheme,reducedMotion:'reduce'});
    await page.setViewportSize({width,height:900});
    check(await dialog.evaluate(el => {const r=el.getBoundingClientRect();return r.left>=0 && r.right<=innerWidth && r.top>=0 && r.bottom<=innerHeight && el.scrollWidth<=el.clientWidth;}), `Viewer fits ${colorScheme}/${width}`);
    check(await dialog.locator('img').evaluate(el => {const r=el.getBoundingClientRect(),s=el.parentElement.getBoundingClientRect();return r.width>100&&r.left>=s.left&&r.right<=s.right+1&&r.bottom<=s.bottom+1;}), `Image fits ${colorScheme}/${width}`);
    await page.screenshot({path:`output/playwright/image-preview/${colorScheme}-${width}.png`});
  }
  await dialog.getByRole('button',{name:'Actual size',exact:true}).click();
  check(await dialog.locator('img').evaluate(el => el.getBoundingClientRect().width) === 1800, 'Actual size preserves image pixels');
  check(await dialog.locator('.attachment-viewer-stage').evaluate(el => el.scrollWidth>el.clientWidth), 'Large image scrolls within the viewer');
  await dialog.getByRole('button',{name:'Fit image',exact:true}).click();
  const [download] = await Promise.all([page.waitForEvent('download'),dialog.getByRole('button',{name:'Download image',exact:true}).click()]);
  const stream = await download.createReadStream(), chunks = []; for await (const chunk of stream) chunks.push(chunk);
  check(Buffer.concat(chunks).toString('base64') === fixture.png, 'Explicit image download preserves bytes');
  check(download.suggestedFilename() === 'release-overview.png', 'Explicit image download preserves name');
  await page.keyboard.press('Escape');
  await dialog.waitFor({state:'detached'});
  check(await opener.evaluate(el => el===document.activeElement), 'Escape restores focus to the image name');
  await opener.press('Enter'); await ready();
  await dialog.getByRole('button',{name:'Close image',exact:true}).focus();
  await page.keyboard.press('Tab');
  check(await page.evaluate(() => !!document.activeElement.closest('.attachment-viewer')), 'Keyboard focus stays inside the modal');
  await page.keyboard.press('Escape');
  const [textDownload] = await Promise.all([page.waitForEvent('download'),page.getByRole('button',{name:'release-notes.txt',exact:true}).click()]);
  check(textDownload.suggestedFilename()==='release-notes.txt', 'Non-image names still download');
  await page.waitForFunction(() => document.querySelector('#issue-attachments').dataset.busy==='false');
  const [imageDownload] = await Promise.all([page.waitForEvent('download'),page.getByRole('button',{name:'Download release-overview.png',exact:true}).click()]);
  check(imageDownload.suggestedFilename()==='release-overview.png', 'Separate row download action still works');
  await page.waitForFunction(() => document.querySelector('#issue-attachments').dataset.busy==='false');
  for (const name of ['broken.png','corrupt.png']) {
    await page.getByRole('button',{name,exact:true}).click();
    await dialog.getByRole('button',{name:'Retry',exact:true}).waitFor();
    check(await dialog.getByRole('alert').isVisible(), `${name} shows a recoverable error`);
    await page.keyboard.press('Escape');
  }
  let fail = true;
  const failure = async route => {
    const operation = route.request().postDataJSON()?.operation;
    if (fail && operation?.action==='attachment' && operation.operation.command==='download') {
      fail=false; await route.fulfill({status:503,json:{ok:false,error:{message:'Synthetic connection lost'}}}); return;
    }
    await route.continue();
  };
  await page.route('**/api/action',failure);
  await opener.click(); await dialog.getByRole('button',{name:'Retry',exact:true}).click(); await ready();
  checks.push('Failed image read retries successfully');
  await page.unroute('**/api/action',failure); await page.keyboard.press('Escape');
  for (const name of ['photo.JPG','image.webp','pixel.gif']) {
    await page.getByRole('button',{name,exact:true}).click();
    await page.waitForFunction(()=>{const img=document.querySelector('.attachment-viewer img');return img?.naturalWidth>0&&!img.hidden;});
    check(await dialog.getByRole('button',{name:'Actual size',exact:true}).isEnabled(), `${name} decodes in the viewer`);
    await page.keyboard.press('Escape');
  }
  const portrait=page.getByRole('button',{name:'portrait-with-a-very-long-filename-for-narrow-phone-screens.svg',exact:true});
  await portrait.click(); await page.waitForFunction(() => document.querySelector('.attachment-viewer img')?.naturalHeight===1800);
  await page.screenshot({path:'output/playwright/image-preview/portrait-phone.png'});
  check(await dialog.evaluate(el=>el.scrollWidth<=el.clientWidth), 'Long name and portrait fit on a narrow phone');
  await page.keyboard.press('Escape');
  await page.evaluate(() => {
    window.previewURLs = new Set();
    const create=URL.createObjectURL.bind(URL), revoke=URL.revokeObjectURL.bind(URL);
    URL.createObjectURL=blob=>{const url=create(blob);previewURLs.add(url);return url;};
    URL.revokeObjectURL=url=>{previewURLs.delete(url);revoke(url);};
  });
  await opener.click(); await ready(); await page.keyboard.press('Escape');
  check(await page.evaluate(()=>previewURLs.size===0), 'Closing releases the image object URL');
  let release, entered;
  const held=new Promise(resolve=>release=resolve), started=new Promise(resolve=>entered=resolve);
  const slow=async route=>{
    const operation=route.request().postDataJSON()?.operation;
    if(operation?.action==='attachment'&&operation.operation.command==='download') {entered();await held;}
    await route.continue();
  };
  await page.route('**/api/action',slow);
  await opener.click(); await started;
  check(await dialog.getByRole('status').innerText()==='Loading image…', 'Slow image reads show loading feedback');
  await dialog.getByRole('button',{name:'Close image',exact:true}).click();
  const response=page.waitForResponse(r=>r.url().endsWith('/api/action')&&r.request().postDataJSON()?.operation?.operation?.command==='download');
  release(); await response; await page.unroute('**/api/action',slow);
  check(await page.evaluate(()=>!document.querySelector('.attachment-viewer')&&previewURLs.size===0), 'Closing during loading ignores the late response');
  await page.locator('#comment-body').fill('Keep this unsaved note.');
  await opener.click(); await ready();
  await dialog.getByRole('button',{name:'Close image',exact:true}).click();
  check(await page.locator('#comment-body').inputValue()==='Keep this unsaved note.', 'Preview preserves the unsent comment');
  await page.locator('#comment-body').fill('');
  await opener.click(); await ready();
  await page.evaluate(()=>{location.hash='project='+encodeURIComponent(model.project.id||model.project);});
  await dialog.waitFor({state:'detached'});
  check(await page.evaluate(()=>previewURLs.size===0), 'Navigation closes the viewer and releases image memory');
  await page.goto(route); await opener.waitFor();
  await page.evaluate(({project,number})=>{
    const root=document.createElement('section');root.id='readonly-preview-fixture';document.body.append(root);
    HeyBossAttachments.mount(root,{project,target:{kind:'issue',id:String(number)},csrf:model.csrf,readonly:true});
  },{project,number:fixture.number});
  const readonly=page.locator('#readonly-preview-fixture');
  await readonly.getByRole('button',{name:'release-overview.png',exact:true}).click();await ready();
  check(await readonly.locator('[data-remove],input[type=file]').count()===0, 'Read-only attachments can preview without write controls');
  await readonly.evaluate(el=>el.remove());await dialog.waitFor({state:'detached'});
  check(await page.evaluate(()=>previewURLs.size===0), 'Removing the owning surface cleans up the viewer');
  const resources=await page.evaluate(async({project,png})=>{
    const action=operation=>api(operation,project);
    const artifact=(await action({action:'artifact',operation:{command:'create',title:'Image review',body:'Review the release image.'}})).artifact;
    const graph=await action({action:'mindmap',operation:{command:'view',node:'image-preview'}});
    const node=graph.node;
    if(!node?.id)throw Error('Missing fixture node');
    for(const target of [{kind:'artifact',id:artifact.id},{kind:'node',id:node.id}])await action({action:'attachment',operation:{command:'upload',target,name:'release-overview.png',data:png}});
    return {artifact:artifact.id,node:node.id};
  },{project,png:fixture.png});
  for(const [kind,path] of Object.entries({artifact:'/artifacts',node:'/mm'})) {
    await page.goto(base+path+'#project='+encodeURIComponent(project)+'&'+kind+'='+resources[kind]);
    await page.getByRole('button',{name:'release-overview.png',exact:true}).first().click();await ready();
    check(await dialog.locator('footer').evaluate(el=>getComputedStyle(el).flexDirection)==='row', `${kind} keeps the compact viewer toolbar`);
    await page.screenshot({path:`output/playwright/image-preview/${kind}-phone.png`});
    await page.keyboard.press('Escape');checks.push(`${kind} uses the shared image viewer`);
  }
  check(errors.length===0, 'No browser script errors: '+errors.join('\n'));
  return {passed:checks.length,checks};
}
