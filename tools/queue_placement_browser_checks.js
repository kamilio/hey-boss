// Run with playwright-cli run-code --filename against serve_queue_placement_fixture.mjs.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  page.setDefaultTimeout(20000);
  const origin = await page.evaluate(() => location.origin);
  const path = await page.evaluate(() => location.pathname === '/issues' ? '/issues' : '/');
  const surface = path === '/issues' ? 'paired' : 'native';
  await page.goto(origin + path);
  await page.reload();
  await page.waitForFunction(() => model.csrf && model.project && model.signature);
  await page.route('**/api/inbox', route => route.fulfill({json:{ok:true,tasks:[],unread_count:0}}));
  const base = origin + path + '#project=named%3AQueue%20placement%20QA';
  for (const scheme of ['light','dark']) for (const width of [1440,768,390,320]) {
    await page.emulateMedia({colorScheme:scheme,reducedMotion:'reduce'});
    await page.setViewportSize({width,height:900});
    await page.goto(base);
    await page.waitForFunction(() => model.signature && model.issues.length >= 5);
    check(JSON.stringify(await page.locator('.issue-row').evaluateAll(rows => rows.slice(0,5).map(r => Number(r.dataset.issueNumber)))) === '[4,2,1,3,5]', `${scheme}/${width}: priority order and draft preserved`);
    await page.locator('[data-issue-number="3"] .issue-title').click();
    await page.locator('#history-toggle').click();
    await page.getByText('Added to the bottom of the queue.',{exact:true}).waitFor();
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),`${scheme}/${width}: placement fits`);
    await page.goto(base+'&issue=2');
    await page.locator('#history-toggle').focus();
    await page.keyboard.press('Enter');
    await page.getByText('MCP recovery worker reserved this issue as the first eligible task in queue order.',{exact:true}).waitFor();
    await page.getByText('Added to the top of the queue.',{exact:true}).waitFor();
    const details = page.getByText('Selection details',{exact:true});
    await details.focus();
    await page.keyboard.press('Enter');
    check(await details.locator('..').evaluate(el => el.open),`${scheme}/${width}: selection details keyboard accessible`);
    check((await details.locator('..').innerText()).includes('"required_tags"'),`${scheme}/${width}: actual filters exposed`);
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),`${scheme}/${width}: expanded diagnostics fit`);
    await page.locator('#activity-timeline').scrollIntoViewIfNeeded();
    await page.screenshot({path:`output/playwright/issue701/${surface}-${scheme}-${width}.png`});
  }
  await page.goto(base);
  await page.locator('#new-issue').click();
  check(!await page.locator('#editor-bottom').isChecked(),'Human editor still defaults to top');
  await page.locator('#editor-subject').fill('Human explicit bottom '+surface);
  await page.locator('#editor-bottom').check();
  await page.locator('#editor-submit').click();
  await page.waitForFunction(() => !document.querySelector('#editor-dialog')?.open && model.issues.some(i=>i.title.startsWith('Human explicit bottom')));
  check((await page.locator('.issue-row').last().innerText()).includes('Human explicit bottom'),'Human bottom choice is honored');
  check(errors.length === 0, 'No browser exceptions: '+errors.join('; '));
  await page.unrouteAll({behavior:'wait'});
  return {checks:checks.length,errors};
}
