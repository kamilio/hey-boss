// playwright-cli --session issue253 run-code --filename tools/long_title_browser_checks.js
// Use tools/serve_long_title_fixture.mjs; run once on native and once on /issues.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.setDefaultTimeout(20000);
  page.on('pageerror', error => errors.push(error.message));
  const base = await page.evaluate(() => location.origin);
  const path = await page.evaluate(() => location.pathname === '/issues' ? '/issues' : '/');
  const surface = path === '/issues' ? 'paired' : 'native';
  await page.goto(base + path);
  await page.waitForFunction(() => model.csrf && model.project);
  const longTitle = 'Restore the connection after waking the laptop. '.repeat(15) + 'Final detail 🦀';
  const open = async () => {
    await page.locator('#quick-issue-open').click();
    await page.waitForFunction(() => document.querySelector('#quick-issue-context').textContent.startsWith('Create in'));
  };
  for (const scheme of ['light','dark']) {
    await page.emulateMedia({colorScheme:scheme,reducedMotion:'reduce'});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:900});
      await open();
      await page.locator('#quick-issue-title').fill(longTitle);
      check(await page.locator('#quick-issue-overflow').isVisible(), `${scheme}/${width}: overflow note visible`);
      check(await page.locator('#quick-issue-submit').isEnabled(), `${scheme}/${width}: creation enabled`);
      check(await page.locator('#quick-issue-dialog').evaluate(el => {
        const r=el.getBoundingClientRect();return r.left>=0 && r.right<=innerWidth && r.bottom<=innerHeight && el.scrollWidth<=el.clientWidth;
      }), `${scheme}/${width}: dialog fits`);
      await page.screenshot({path:`output/playwright/issue253/${surface}-${scheme}-${width}.png`});
      await page.keyboard.press('Escape');
    }
  }
  await open();
  check(await page.locator('#quick-issue-title').inputValue() === longTitle, 'Dismissal preserves the whole long draft');
  await page.locator('#quick-issue-title').fill('é'.repeat(256));
  check(!await page.locator('#quick-issue-overflow').isVisible(), 'Exactly 512 UTF-8 bytes does not overflow');
  await page.locator('#quick-issue-title').fill('é'.repeat(257));
  check(await page.locator('#quick-issue-overflow').isVisible(), 'Unicode uses bytes rather than character count');
  await page.locator('#quick-issue-title').fill('界'.repeat(1400));
  check(await page.locator('#quick-issue-title').inputValue() === '界'.repeat(1400), 'Quick Add accepts pastes beyond its old character cap');
  await page.locator('#quick-issue-submit').click();
  await page.waitForFunction(() => !document.querySelector('#quick-issue-dialog').open);
  await page.locator('#quick-issue-status a').click();
  await page.waitForFunction(() => model.detail?.issue);
  const unicode = await page.evaluate(() => model.detail.issue);
  check(unicode.title === '界'.repeat(170) && unicode.body === '界'.repeat(1230), 'Quick Add persists all Unicode overflow');

  await page.setViewportSize({width:1440,height:900});
  await page.goto(base + path);
  await page.waitForFunction(() => model.csrf && model.project);
  await page.locator('#new-issue').click();
  await page.locator('#editor-subject').fill(longTitle);
  await page.locator('#editor-body').fill('Existing **Markdown** description.');
  check(await page.locator('#editor-subject').inputValue() === longTitle, 'Full editor does not truncate pasted text');
  await page.screenshot({path:`output/playwright/issue253/${surface}-full-editor.png`});
  const saved = page.waitForResponse(response => response.url().endsWith('/api/action') && response.request().postDataJSON()?.operation.action === 'create');
  await page.locator('#editor-submit').click();
  const createdNumber = (await (await saved).json()).issue.number;
  await page.locator(`[data-issue-number="${createdNumber}"] .issue-title`).click();
  await page.waitForFunction(number => model.detail?.issue.number === number, createdNumber);
  const created = await page.evaluate(() => model.detail.issue);
  check(await page.evaluate(title => new TextEncoder().encode(title).length <= 512, created.title), 'Saved title stays within the byte limit');
  check(created.title + ' ' + created.body === longTitle + '\n\nExisting **Markdown** description.', 'Word split preserves content and existing description');
  await page.screenshot({path:`output/playwright/issue253/${surface}-saved-detail.png`});
  await page.reload();
  await page.waitForFunction(() => model.detail?.issue);
  check(await page.evaluate(body => model.detail.issue.body === body, created.body), 'Description survives reload');

  let first=true; const requests=[];
  await page.route('**/api/action', async route => {
    const data=route.request().postDataJSON();
    if(data.operation.action!=='create') return route.continue();
    requests.push(data);
    if(first) {first=false;await route.fetch();return route.abort('failed');}
    return route.continue();
  });
  await open();await page.locator('#quick-issue-title').fill(longTitle+' Retry marker');
  await page.locator('#quick-issue-submit').click();
  await page.waitForFunction(() => !document.querySelector('#quick-issue-error').hidden && !document.querySelector('#quick-issue-title').disabled);
  check(await page.locator('#quick-issue-title').inputValue() === longTitle+' Retry marker', 'Lost response preserves all text');
  await page.locator('#quick-issue-submit').click();
  await page.waitForFunction(() => !document.querySelector('#quick-issue-dialog').open);
  check(requests.length===2 && requests[0].request_id===requests[1].request_id, 'Retry reuses its mutation ID');
  await page.unroute('**/api/action');
  check(errors.length===0, 'No uncaught browser errors');
  return {surface,passed:checks.length,checks};
}
