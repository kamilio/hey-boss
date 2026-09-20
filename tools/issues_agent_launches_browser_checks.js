async page => {
  const checks = [], errors = [], engine = page.context().browser().browserType().name();
  const check = (ok, label) => {if (!ok) throw Error(label);checks.push(label);};
  page.on('pageerror', e => errors.push(e.message));
  const url = 'http://127.0.0.1:4794/#project=named%3ALaunch%20QA';
  await page.goto(url, {waitUntil:'domcontentloaded'});
  await page.reload({waitUntil:'domcontentloaded'});
  const fits = async selector => page.locator(selector).evaluateAll(elements => elements.every(el => {
    const r = el.getBoundingClientRect();return r.left >= 0 && r.right <= innerWidth && el.scrollWidth <= el.clientWidth;
  }));
  for (const theme of ['light','dark']) {
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:1000});
      await page.goto(url);
      await page.locator('.issue-row').nth(3).waitFor();
      check(await page.locator('.issue-row').nth(0).locator('.agent-launch-count').count() === 0,'Zero hidden in list');
      check(await page.locator('#issue-list .agent-launch-count').allTextContents().then(values => values.length === 3 && values[0].startsWith('1 agent launch') && values[1].startsWith('12 agent launches') && values[2].startsWith('1234 agent launches')),`${theme}/${width}: counts correct`);
      check(await fits('.issue-row'),`${theme}/${width}: list fits`);
      const badge = page.locator('#issue-list .agent-launch-count').nth(0);
      await badge.hover();
      check(await badge.locator('.agent-launch-help').isVisible(),`${theme}/${width}: hover help`);
      check(await fits('.agent-launch-count:hover .agent-launch-help'),`${theme}/${width}: hover help fits`);
      await badge.locator('.agent-launch-help').hover();
      check(await badge.locator('.agent-launch-help').isVisible(),'Help stays open when hovered');
      await page.screenshot({path:`output/playwright/issue44/${engine}-${theme}-${width}-list.png`});
      await badge.focus();
      await page.mouse.move(0,0);
      check(await badge.locator('.agent-launch-help').isVisible(),`${theme}/${width}: keyboard help`);
      check((await badge.getAttribute('aria-label')).includes('including retries and resumed sessions'), 'Accessible explanation');
      await page.keyboard.press('Escape');
      check(!await badge.locator('.agent-launch-help').isVisible(),'Escape dismisses help');
      await page.locator('.issue-title').nth(2).click();
      await page.locator('.sidebar .agent-launch-count').waitFor();
      const detail = page.locator('.sidebar .agent-launch-count');
      check((await detail.textContent()).startsWith('12 agent launches'), 'Detail matches list');
      await detail.scrollIntoViewIfNeeded();
      await detail.hover();
      check(await fits('.agent-launch-help:visible'), `${theme}/${width}: detail help fits`);
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),`${theme}/${width}: detail has no horizontal scroll`);
      await page.screenshot({path:`output/playwright/issue44/${engine}-${theme}-${width}-detail.png`});
      await page.goto(url+'&issue=1');
      await page.locator('.sidebar .agent-launch-count').waitFor();
      check((await page.locator('.sidebar .agent-launch-count').textContent()).startsWith('0 agent launches'),'Zero available in detail');
    }
  }
  await page.route('**/api/action', async route => {
    if (route.request().postDataJSON().operation.action !== 'list') return route.continue();
    const response = await route.fetch(), body = await response.json();
    body.issues[0].agent_launch_count = 2;
    await route.fulfill({response,json:body});
  });
  await page.goto(url);
  await page.locator('#issue-list .agent-launch-count').nth(0).waitFor();
  const first = page.locator('#issue-list .agent-launch-count').nth(0);
  await first.hover();
  check(await first.locator('.agent-launch-help').evaluate(el => {
    const r=el.getBoundingClientRect();return el.contains(document.elementFromPoint(r.left+r.width/2,r.top+r.height/2));
  }),'First row help is not clipped by the list');
  await page.unroute('**/api/action');
  await page.goto(url+'&issue=3');
  await page.locator('.sidebar .agent-launch-count').waitFor();
  await page.getByLabel('Your comment').fill('Keep this unsent comment while launch metadata refreshes.');
  await page.route('**/api/action', async route => {
    if (route.request().postDataJSON().operation.action !== 'view') return route.continue();
    const response = await route.fetch();
    const body = await response.json();
    body.issue.agent_launch_count += 1;
    await route.fulfill({response,json:body});
  });
  await page.evaluate(async () => {while(model.polling) await new Promise(r=>setTimeout(r,10));await refresh(false);});
  check((await page.locator('.sidebar .agent-launch-count').textContent()).startsWith('13 agent launches'),'Open detail count refreshes without revision change');
  check(await page.getByLabel('Your comment').inputValue() === 'Keep this unsent comment while launch metadata refreshes.','Refresh preserves unsent comment');
  await page.unroute('**/api/action');
  check(errors.length === 0,`No browser errors: ${errors}`);
  return {checks:checks.length,errors};
}
