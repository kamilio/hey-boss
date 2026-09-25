// Run with playwright-cli against an isolated issue web server.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/api/inbox', route => route.fulfill({json:{ok:true,tasks:[],unread:0}}));
  await page.waitForFunction(() => model.csrf && model.project);
  const origin = await page.evaluate(() => location.origin);
  const path = await page.evaluate(() => location.pathname === '/issues' ? '/issues' : '/');
  const surface = path === '/issues' ? 'paired' : 'native';
  const output = 'output/playwright/issue252/' + surface;

  const fixture = await page.evaluate(async () => {
    const suffix = crypto.randomUUID();
    const source = await api({action:'create',title:'Move an issue to the right project',body:'## Release notes\nPreserve **Markdown**, comments and labels.',labels:['ready']}, 'Transfer source '+suffix);
    const target = await api({action:'create',title:'Existing destination issue',body:'',labels:[]}, 'Transfer destination '+suffix);
    await api({action:'comment',number:1,body:'A comment that follows the issue.'},source.project.id);
    return {source:source.project.id,target:target.project.id};
  });
  const open = async (project, number) => {
    await page.goto(origin+path+'#project='+encodeURIComponent(project)+'&issue='+number);
    await page.waitForFunction(({project,number}) => model.project?.id===project && model.detail?.issue.number===number && !!document.querySelector('.issue-overflow'),{project,number});
  };
  const menu = async () => { await page.locator('.issue-overflow summary').click(); await page.locator('[data-transfer]').waitFor(); };
  const dialog = async () => {
    await menu(); await page.locator('[data-transfer]').click();
    await page.waitForFunction(() => document.querySelector('#transfer-dialog').open && !document.querySelector('#transfer-project').disabled);
  };
  const overflow = async name => check(await page.evaluate(() => document.documentElement.scrollWidth<=innerWidth),name);
  await page.setViewportSize({width:1440,height:1000});
  await open(fixture.source,1);
  for (const component of ['progress','attachments','artifacts','subtasks']) {
    const attempts = await page.evaluate(async component => {
      const target = component === 'attachments' ? HeyBossAttachments : component === 'artifacts' ? HeyBossArtifacts : component === 'subtasks' ? IssueSubtasks : null;
      const key = component === 'subtasks' ? 'rendered' : 'mount';
      const original = target ? target[key] : mountIssueProgress;
      let attempts = 0;
      const fail = () => {attempts++;throw Error('Synthetic ' + component + ' failure');};
      try {
        if (target) target[key] = fail; else mountIssueProgress = fail;
        detailCache.clear();
        await renderRoute();
        return attempts;
      }
      finally {if (target) target[key] = original; else mountIssueProgress = original;}
    }, component);
    check(attempts > 0, 'Exercises failed ' + component);
    if (['attachments','artifacts'].includes(component)) check(await page.getByRole('button', {name:'Try again', exact:true}).isVisible(), 'Section failure is visible and retryable: ' + component);
    await dialog();
    check(await page.locator('#transfer-dialog').isVisible(), 'Move still works after failed ' + component);
    await page.keyboard.press('Escape');
    if (['attachments','artifacts'].includes(component)) {
      await page.screenshot({path:output+'-'+component+'-failure.png'});
      await page.getByRole('button', {name:'Try again', exact:true}).click();
      await page.waitForFunction(() => !document.querySelector('#detail-view [data-reload]'));
      check(await page.locator('.issue-overflow summary').isVisible(), 'Retry preserves actions: ' + component);
    } else await page.evaluate(() => renderDetail(model.detail));
  }
  await menu(); await page.screenshot({path:output+'-desktop-menu.png'});
  await page.keyboard.press('Escape');
  check(!await page.locator('.issue-overflow').evaluate(e=>e.open),'Escape closes overflow menu');
  await page.locator('.issue-overflow summary').focus(); await page.keyboard.press('Enter');
  await page.keyboard.press('Tab'); await page.keyboard.press('Enter');
  await page.waitForFunction(() => document.querySelector('#transfer-dialog').open && !document.querySelector('#transfer-project').disabled);
  check(await page.locator('#transfer-submit').isDisabled(),'Move requires a destination');
  check(await page.locator('#transfer-project option').evaluateAll((options,source)=>!options.some(o=>o.value===source),fixture.source),'Current project is excluded');
  await page.locator('#transfer-project').selectOption(fixture.target);
  await page.screenshot({path:output+'-desktop-dialog.png'});
  await overflow('Desktop has no horizontal overflow');
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => document.querySelector('.issue-overflow summary') === document.activeElement);
  check(true,'Cancel restores keyboard focus');
  await page.locator('#comment-body').fill('Unsaved comment follows this move');
  await dialog();
  await page.evaluate(async project => api({action:'edit',number:1,title:'Updated title in another session',body:null,add_labels:[],remove_labels:[],if_version:null},project),fixture.source);
  await page.locator('#transfer-project').selectOption(fixture.target); await page.locator('#transfer-submit').click();
  await page.locator('#transfer-error').waitFor();
  check((await page.locator('#transfer-error').innerText()).includes('Issue changed'),'Stale revision fails with a readable error');
  await page.screenshot({path:output+'-stale-error.png'});
  await page.locator('#transfer-dialog [data-transfer-cancel]').last().click();
  await page.evaluate(() => refresh(false));
  if (await page.locator('[data-reload]').count()) await page.locator('[data-reload]').click();
  await page.waitForFunction(() => model.detail?.issue.title==='Updated title in another session');
  await dialog(); await page.locator('#transfer-project').selectOption(fixture.target);
  // The server commits, but the response is lost. Retry must use the same key.
  let lost = false;
  const loseResponse = async route => {
    const request = route.request();
    if (!lost && request.method()==='POST' && request.postDataJSON()?.operation?.action==='transfer') {
      lost=true; await route.fetch(); await route.abort('failed');
    } else await route.continue();
  };
  await page.route('**/api/action',loseResponse);
  await page.locator('#transfer-submit').click(); await page.locator('#transfer-error').waitFor();
  check(lost,'Exercises a lost response after commit');
  await page.unroute('**/api/action',loseResponse);
  await page.locator('#transfer-submit').click();
  await page.waitForFunction(target => model.project?.id===target && model.detail?.issue.number===2,fixture.target);
  check(await page.locator('#comment-body').inputValue()==='Unsaved comment follows this move','Unsaved comment moves with the issue');
  check((await page.locator('#comments').innerText()).includes('A comment that follows'),'Saved comments follow the issue');
  check(await page.evaluate(() => model.detail.issue.labels.includes('ready')),'Labels follow the issue');
  check(!await page.locator('#transfer-dialog').evaluate(e=>e.open),'Successful move closes dialog');
  const destinationCount = await page.evaluate(async project => (await api({action:'list',state:'open',mine:false,unassigned:false,labels:[],search:null,limit:100,offset:0},project)).issues.length,fixture.target);
  check(destinationCount===2,'Lost response retry creates exactly one destination issue');
  await page.goto(origin+path+'#project='+encodeURIComponent(fixture.source)+'&issue=1');
  await page.waitForFunction(target => model.project?.id===target && model.detail?.issue.number===2,fixture.target);
  check(true,'Original issue link redirects to the new location');
  await page.setViewportSize({width:390,height:844});
  await menu(); await page.screenshot({path:output+'-mobile-menu.png'});
  await page.locator('[data-transfer]').click();
  await page.waitForFunction(() => !document.querySelector('#transfer-project').disabled);
  await page.locator('#transfer-project').selectOption(fixture.source);
  await page.screenshot({path:output+'-mobile-dialog.png'});
  await overflow('Mobile has no horizontal overflow');
  const fits = await page.locator('#transfer-dialog').boundingBox();
  check(fits.x>=0 && fits.x+fits.width<=390 && fits.y>=0 && fits.y+fits.height<=844,'Mobile dialog fits the viewport');
  await page.emulateMedia({colorScheme:'dark'});
  await page.screenshot({path:output+'-mobile-dark-dialog.png'});
  await page.locator('#transfer-dialog [data-transfer-cancel]').last().click();
  await page.setViewportSize({width:1440,height:1000}); await dialog();
  await page.locator('#transfer-project').selectOption(fixture.source);
  await page.screenshot({path:output+'-desktop-dark-dialog.png'});
  await page.keyboard.press('Escape');
  for (const scheme of ['light','dark']) for (const width of [320,768]) {
    await page.emulateMedia({colorScheme:scheme});
    await page.setViewportSize({width,height:844});
    await menu();
    check(await page.locator('.issue-overflow-menu').evaluate(el => {const r=el.getBoundingClientRect();return r.left>=0 && r.right<=innerWidth;}), 'Menu fits '+scheme+'/'+width);
    await page.locator('[data-transfer]').click();
    await page.waitForFunction(() => !document.querySelector('#transfer-project').disabled);
    await page.locator('#transfer-project').selectOption(fixture.source);
    await page.screenshot({path:output+'-'+scheme+'-'+width+'.png'});
    check(await page.locator('#transfer-dialog').evaluate(el => {const r=el.getBoundingClientRect();return r.left>=0 && r.right<=innerWidth && r.top>=0 && r.bottom<=innerHeight;}), 'Dialog fits '+scheme+'/'+width);
    await overflow('No horizontal overflow '+scheme+'/'+width);
    await page.keyboard.press('Escape');
  }
  check(errors.length===0,'No browser JavaScript errors');
  return {passed:checks.length,checks};
}
