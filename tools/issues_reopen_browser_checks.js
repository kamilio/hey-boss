// Run with playwright-cli run-code --filename against serve_issue_reopen_fixture.mjs.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.setDefaultTimeout(90000);
  page.setDefaultNavigationTimeout(90000);
  page.removeAllListeners('dialog');
  page.on('dialog', async dialog => {
    if (dialog.type() !== 'beforeunload') throw Error('Unexpected dialog: ' + dialog.type());
    await dialog.accept();
  });
  page.on('pageerror', error => errors.push(error.message));
  const base = await page.evaluate(() => location.origin);
  if (!['http://127.0.0.1:59661', 'http://127.0.0.1:52061'].includes(base)) throw Error('Synthetic fixture required');
  const paired = base.endsWith(':52061'), path = paired ? '/issues' : '/';
  const surface = paired ? 'paired' : 'native';
  await page.route('**/api/inbox', route => route.fulfill({json:{ok:true,tasks:[],unread_count:0}}));
  if (paired) {
    const {code} = await (await page.request.get(base + '/fixture-pairing')).json();
    await page.request.post(base + '/api/pair', {data:{code}});
    // Compare the same immutable release assets on both surfaces, even if the
    // shared checkout's generated mobile directory has a concurrent edit.
    for (const name of ['app.js','app.css']) {
      const response = await page.request.get('http://127.0.0.1:59661/' + name);
      const body = await response.body();
      await page.route(base + '/issue-web/' + name, route => route.fulfill({body,contentType:name.endsWith('.js')?'text/javascript':'text/css'}));
    }
  }
  await page.goto(base + path, {waitUntil:'domcontentloaded'});
  await page.waitForFunction(() => model.csrf && model.project && model.signature);
  const project = 'named:Reopen QA ' + Date.now();
  const action = operation => page.evaluate(({operation, project}) => api(operation, project), {operation, project});
  const view = async number => (await action({action:'view',number})).issue;
  const detail = async number => {
    await page.goto(base + path + '#project=' + encodeURIComponent(project) + '&issue=' + number, {waitUntil:'domcontentloaded'});
    await page.waitForFunction(number => model.detail?.issue.number === number, number);
  };
  await action({action:'create',title:'Prepare the shared API',body:'Finish this dependency before the rollout.',labels:[]});
  await action({action:'create',title:'Roll out the updated issue workflow',body:'Reopen this issue while its dependency is unfinished.',labels:[]});
  await action({action:'set_blockers',number:2,blockers:[1],force:false});
  await action({action:'close',number:2,force:false});
  const closed = await view(2);
  await detail(2);
  const reopen = page.getByRole('button',{name:'Reopen issue',exact:true});
  check(await reopen.isEnabled(), 'Closed issue with dependencies can reopen');
  await page.locator('#comment-body').fill('Keep this unsent discussion note.');
  let sent;
  const fail = async route => {
    const body = route.request().postDataJSON();
    if (body?.operation?.action !== 'reopen') return route.continue();
    sent = body;
    await route.fulfill({status:500,json:{ok:false,error:{code:'io_error',message:'Synthetic reopen failure'}}});
  };
  await page.route('**/api/action', fail);
  await reopen.click();
  await page.getByText('Synthetic reopen failure',{exact:true}).waitFor();
  check(await reopen.isEnabled(), 'Failed reopen remains retryable');
  check((await view(2)).state === 'closed', 'Failed reopen preserves closed state');
  check(sent.operation.if_version === closed.version, 'Browser sends the displayed version guard');
  await page.unroute('**/api/action', fail);
  await reopen.focus();
  await page.keyboard.press('Enter');
  await page.locator('.state-pill.blocked').waitFor();
  const reopened = await view(2);
  check(reopened.closed_at === null && reopened.closed_by === null, 'Reopen clears closure metadata');
  check(JSON.stringify(reopened.blocker_numbers) === JSON.stringify(closed.blocker_numbers), 'Reopen retains dependency links');
  check(JSON.stringify(reopened.blocked_by) === JSON.stringify(closed.blocked_by), 'Reopen retains unfinished dependency details');
  check(await reopen.isDisabled(), 'Reopen cannot bypass dependency waiting');
  check((await page.locator('.blocked-notice').innerText()).includes('Workers won’t pick up'), 'Waiting state explains paused pickup');
  check(await page.locator('#comment-body').inputValue() === 'Keep this unsent discussion note.', 'Reopen preserves unsent discussion');
  check((await page.locator('#toast').innerText()).includes('waiting for dependencies'), 'Reopen feedback explains dependency waiting');
  for (const scheme of ['light','dark']) {
    await page.emulateMedia({colorScheme:scheme,reducedMotion:'reduce'});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:900});
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `No horizontal overflow ${scheme}/${width}`);
      check(await page.locator('.blocked-notice').evaluate(el => {const r=el.getBoundingClientRect();return r.left>=0 && r.right<=innerWidth;}), `Waiting notice fits ${scheme}/${width}`);
      await page.screenshot({path:`output/playwright/issue361/${surface}-${scheme}-${width}.png`,fullPage:true});
    }
  }
  await page.reload();
  await page.locator('.state-pill.blocked').waitFor();
  check((await view(2)).state === 'blocked', 'Dependency waiting survives reload');
  await action({action:'close',number:1,force:false});
  await page.reload();
  await page.locator('.state-pill.open').waitFor();
  check((await view(2)).state === 'open', 'Finishing dependency automatically reopens pickup');
  check(await page.locator('.blocked-notice').count() === 0, 'Resolved dependency removes waiting notice');
  check((await view(2)).blocker_numbers[0] === 1, 'Completed dependency link stays attached');
  await action({action:'close',number:2,force:false});
  await page.reload();
  await reopen.click();
  await page.locator('.state-pill.open').waitFor();
  check((await view(2)).state === 'open', 'Reopening with finished dependencies remains immediately open');
  check(errors.length === 0, 'No browser errors: ' + errors.join('; '));
  return {surface,checks:checks.length,passed:checks};
}
