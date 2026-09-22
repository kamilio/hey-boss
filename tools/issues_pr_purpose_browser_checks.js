// Run against an isolated issue web server on port 4782 with playwright-cli run-code.
async page => {
  page.setDefaultTimeout(10000);
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  page.on('dialog', dialog => dialog.accept());
  await page.goto('http://127.0.0.1:4782/', {waitUntil:'domcontentloaded'});
  await page.waitForFunction(() => model.csrf);
  const seed = await page.evaluate(async () => {
    const created = await api({action:'create', title:'Separate the runtime fix from investigation', body:'The fix restores runtime behavior. The investigation provides supporting evidence; review both PRs.', labels:[]}, 'PR Purpose QA');
    const project = created.project.id, number = created.issue.number;
    for (const [id, purpose] of [[15064,'fix'],[15006,'supporting-evidence'],[15065,'prerequisite'],[15066,'unspecified']]) {
      await api({action:'add_pull_request',number,url:`https://github.com/example/runtime/pull/${id}`,purpose},project);
    }
    return {project,number};
  });
  const detailURL = `http://127.0.0.1:4782/#project=${encodeURIComponent(seed.project)}&issue=${seed.number}`;
  await page.goto(detailURL, {waitUntil:'domcontentloaded'});
  await page.waitForSelector('[data-pr-purpose]');
  check(await page.locator('[data-pr-purpose]').count() === 4, 'All four purposes appear on attached links');
  check(await page.locator('[data-pr-purpose$="/15066"]').inputValue() === 'unspecified', 'Details retain the unspecified purpose selector');
  const evidence = page.locator('[data-pr-purpose$="/15006"]');
  check(await evidence.inputValue() === 'supporting-evidence', 'Investigation is clearly classified');
  const before = await page.evaluate(async ({project,number}) => (await api({action:'view',number},project)).issue, seed);
  await evidence.selectOption('prerequisite');
  await page.waitForFunction(() => document.querySelector('[data-pr-purpose$="/15006"]')?.dataset.savedPurpose === 'prerequisite');
  const after = await page.evaluate(async ({project,number}) => (await api({action:'view',number},project)).issue, seed);
  const oldPR = before.pull_requests.find(pr => pr.url.endsWith('/15006'));
  const newPR = after.pull_requests.find(pr => pr.url.endsWith('/15006'));
  check(newPR.added_by === oldPR.added_by && newPR.created_at === oldPR.created_at, 'Reclassification preserves attachment history');
  check(after.state === before.state && after.assignee === before.assignee, 'Reclassification preserves lifecycle and ownership');
  await page.reload({waitUntil:'domcontentloaded'});
  await page.waitForSelector('[data-pr-purpose]');
  check(await evidence.inputValue() === 'prerequisite', 'Classification survives reload');
  // A server failure must restore the saved value and expose an actionable error.
  await page.route('**/api/action', async route => {
    if (route.request().postDataJSON()?.operation?.action === 'classify_pull_request') {
      await route.fulfill({status:409,contentType:'application/json',body:JSON.stringify({ok:false,error:{code:'conflict',message:'Test conflict: refresh and retry.'}})});
    } else await route.continue();
  });
  await evidence.selectOption('fix');
  await page.locator('#pr-error').waitFor({state:'visible'});
  check(await evidence.inputValue() === 'prerequisite' && await evidence.isEnabled(), 'Failed classification restores saved value and enables retry');
  await page.unroute('**/api/action');
  await evidence.selectOption('supporting-evidence');
  await page.waitForFunction(() => document.querySelector('[data-pr-purpose$="/15006"]')?.dataset.savedPurpose === 'supporting-evidence');
  check(await page.locator('#pr-error').isHidden(), 'Successful retry clears stale error');
  await page.locator('#pr-url').fill('https://github.com/example/runtime/pull/15067');
  await page.locator('#pr-purpose').selectOption('fix');
  await page.getByRole('button',{name:'Attach PR',exact:true}).click();
  await page.waitForSelector('[data-pr-purpose$="/15067"]');
  check(await page.locator('[data-pr-purpose$="/15067"]').inputValue() === 'fix', 'Attach records chosen purpose');
  const fits = async () => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth && [...document.querySelectorAll('.pr-link,.pr-purpose-field select')].every(el => { const r=el.getBoundingClientRect(); return r.width>0 && r.left>=0 && r.right<=innerWidth; }));
  for (const [name,width,height,scheme] of [['desktop-light',1440,1000,'light'],['desktop-dark',1440,1000,'dark'],['mobile-light',390,844,'light'],['mobile-dark',390,844,'dark'],['narrow-mobile',320,760,'light']]) {
    await page.setViewportSize({width,height});
    await page.emulateMedia({colorScheme:scheme});
    await page.locator('.pr-links').scrollIntoViewIfNeeded();
    check(await fits(), `${name}: PR controls fit without horizontal overflow`);
    await page.screenshot({path:`output/playwright/issue45/${name}.png`,fullPage:true});
  }
  await page.setViewportSize({width:1440,height:1000});
  // Native selects stay accessible to the keyboard.
  await evidence.focus();
  await page.keyboard.press('p');
  await page.keyboard.press('Tab');
  await page.waitForFunction(() => document.querySelector('[data-pr-purpose$="/15006"]')?.dataset.savedPurpose === 'prerequisite');
  check(await evidence.inputValue() === 'prerequisite', 'Keyboard can change the PR purpose');
  await evidence.selectOption('supporting-evidence');
  await page.waitForFunction(() => document.querySelector('[data-pr-purpose$="/15006"]')?.dataset.savedPurpose === 'supporting-evidence');
  await page.locator('[data-remove-pr$="/15067"]').click();
  await page.waitForFunction(() => document.querySelectorAll('[data-pr-purpose]').length === 4 && !document.querySelector('[data-pr-purpose$="/15067"]'));
  check(await page.locator('[data-pr-purpose]').count() === 4, 'Removal preserves other classified links');
  await page.goto(`http://127.0.0.1:4782/#project=${encodeURIComponent(seed.project)}`, {waitUntil:'domcontentloaded'});
  await page.waitForSelector('.issue-pr-link');
  const issueLinks = page.locator(`.issue-row[data-issue-number="${seed.number}"] .issue-pr-link`);
  check((await issueLinks.allTextContents()).some(text => text.includes('Supporting evidence')), 'List keeps evidence visible with its purpose');
  check(await issueLinks.count() === 4, 'Every attached PR remains visible in list');
  const unspecifiedLink = page.locator(`.issue-row[data-issue-number="${seed.number}"] .issue-pr-link[href$="/15066"]`);
  check((await unspecifiedLink.textContent()).trim() === 'example/runtime#15066', 'Unspecified PR shows only its identifier');
  check(await unspecifiedLink.locator('.pr-purpose-label').count() === 0, 'Unspecified PR has no empty badge');
  check(!(await unspecifiedLink.getAttribute('aria-label')).includes('Unspecified') && !(await unspecifiedLink.getAttribute('title')).includes('Unspecified'), 'Unspecified purpose is omitted from accessible name and tooltip');
  check(await issueLinks.locator('.pr-purpose-label').count() === 3, 'Classified PRs retain their badges');
  for (const [width,scheme] of [[1440,'light'],[1440,'dark'],[390,'light'],[390,'dark'],[320,'light'],[320,'dark']]) {
    await page.setViewportSize({width,height:1000});await page.emulateMedia({colorScheme:scheme});
    check(await page.evaluate(() => document.documentElement.scrollWidth<=innerWidth), `List ${width}px fits`);
    check(await issueLinks.locator('.pr-link-title').evaluateAll(els => els.every(el => {const range=document.createRange();range.selectNodeContents(el);return range.getClientRects().length===1;})), `List ${width}px keeps PR identifiers intact`);
    await page.screenshot({path:`output/playwright/issue45/list-${width}-${scheme}.png`,fullPage:true});
  }
  const link = issueLinks.first();
  check(await link.getAttribute('target') === '_blank' && (await link.getAttribute('rel')).includes('noopener'), 'PR links open safely in another tab');
  check(errors.length === 0, 'No browser exceptions');
  return {passed:checks.length,checks,seed};
}
