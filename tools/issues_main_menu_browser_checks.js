// Run with playwright-cli against an isolated issue web server.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/api/inbox', route => route.fulfill({
    json: {ok:true, tasks:[], unread:0},
  }));
  await page.waitForFunction(() => typeof model !== 'undefined' && model.csrf && model.project);
  const origin = await page.evaluate(() => location.origin);
  const project = await page.evaluate(async () => {
    const value = await api({action:'create', title:'Return to the issue list',
      body:'The Issues menu should return to the list while keeping your filters.', labels:['ready']},
      'Main menu QA ' + crypto.randomUUID());
    await api({action:'close', number:1, force:false}, value.project.id);
    return value.project.id;
  });
  const open = async filtered => {
    const params = new URLSearchParams({project, issue:'1', ...(filtered ?
      {state:'closed', owner:'unassigned', label:'ready', search:'Return'} : {})});
    await page.goto(origin + '/#' + params);
    await page.waitForFunction(project => model.project?.id === project &&
      model.detail?.issue.number === 1 && !document.querySelector('#detail-view').hidden, project);
  };
  const list = async () => {
    await page.waitForFunction(() => !model.route.issue &&
      document.querySelector('#detail-view').hidden && !document.querySelector('#list-view').hidden);
    check(!new URL(page.url()).hash.includes('issue='), 'List URL clears the issue selection');
    check(await page.locator('#nav-issues').getAttribute('aria-current') === 'page', 'Issues retains its selected navigation state');
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'No horizontal overflow');
  };
  await page.setViewportSize({width:1440, height:1000});
  await open(true);
  await page.locator('#comment-body').fill('Saved menu navigation draft');
  await page.screenshot({path:'output/playwright/issue41-desktop-detail.png', fullPage:true});
  await page.locator('#nav-issues').click();
  await list();
  await page.waitForFunction(() => model.issues.length === 1);
  check(await page.evaluate(project => model.route.project === project &&
    model.route.state === 'closed' && model.route.owner === 'unassigned' &&
    model.route.label === 'ready' && model.route.search === 'Return', project), 'Menu click preserves project and list filters');
  await page.screenshot({path:'output/playwright/issue41-desktop-list.png', fullPage:true});
  await page.locator('.issue-title').click();
  await page.locator('#comment-body').waitFor();
  check(await page.locator('#comment-body').inputValue() === 'Saved menu navigation draft', 'Returning to the issue restores its comment draft');
  await page.locator('#nav-issues').focus();
  await page.keyboard.press('Enter');
  await list();
  check(true, 'Keyboard Enter returns from detail to list');
  await page.goBack();
  await page.locator('#comment-body').waitFor();
  check(true, 'Browser Back restores issue detail');
  await page.goForward();
  await list();
  check(true, 'Browser Forward restores the list');
  await page.locator('#nav-inbox').click();
  await page.waitForFunction(() => model.route.view === 'inbox');
  await page.locator('#nav-issues').click();
  await list();
  check(true, 'Inbox to Issues still opens the list');
  await page.setViewportSize({width:390, height:844});
  await page.emulateMedia({colorScheme:'dark', reducedMotion:'reduce'});
  await open(false);
  await page.screenshot({path:'output/playwright/issue41-phone-dark-detail.png', fullPage:true});
  await page.locator('#nav-issues').tap();
  await list();
  await page.screenshot({path:'output/playwright/issue41-phone-dark-list.png', fullPage:true});
  check(true, 'Phone tap returns a deep link to the default list');
  await page.emulateMedia({colorScheme:'light'});
  await open(true);
  await page.locator('#nav-issues').tap();
  await list();
  await page.waitForFunction(() => model.issues.length === 1);
  await page.screenshot({path:'output/playwright/issue41-phone-light-list.png', fullPage:true});
  check(errors.length === 0, 'No JavaScript runtime errors: ' + errors.join('; '));
  return {passed:checks.length, checks};
}
