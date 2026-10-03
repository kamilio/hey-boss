// Playwright CLI run-code after requirements_handoff_install_checks.mjs --serve.
// Create /tmp/hb-handoff-qa first; remove screenshots after visual inspection.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  // This disposable fixture has no notification daemon.
  await page.route('**/api/inbox', route => route.fulfill({json:{ok:true,tasks:[],unread_count:0}}));
  await page.reload();
  await page.waitForFunction(() => model.project?.name === 'Requirements handoff QA');
  const base = await page.evaluate(() => location.origin);
  const project = encodeURIComponent('named:Requirements handoff QA');
  for (const scheme of ['light','dark']) {
    await page.emulateMedia({colorScheme:scheme});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:width > 768 ? 1000 : 844});
      await page.goto(`${base}/#project=${project}&state=ready`);
      const link = page.getByRole('link',{name:'Published delivery awaiting review',exact:true});
      await link.waitFor();
      check(await page.locator('.issue-row').count() === 1, `One Ready delivery ${scheme}/${width}`);
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `List fits ${scheme}/${width}`);
      await page.screenshot({path:`/tmp/hb-handoff-qa/${scheme}-${width}-list.png`,fullPage:true});
      await link.click();
      await page.waitForFunction(() => model.detail?.issue.number === 1);
      await page.waitForFunction(() => document.querySelector('#comments')?.textContent.includes('preserved the delivery handoff'));
      check(await page.locator('.state-pill.ready').innerText() === 'Ready', `Ready state ${scheme}/${width}`);
      check(await page.getByRole('combobox',{name:'Assignment',exact:true}).inputValue() === 'github', `Watcher owns delivery ${scheme}/${width}`);
      const detail = await page.evaluate(() => model.detail.issue);
      check(detail.agent_launch_count === 1 && detail.closed_at === null, `One launch, still unresolved ${scheme}/${width}`);
      check((await page.locator('#comments').innerText()).includes('Published delivery details after watcher handoff.'), `Final notes retained ${scheme}/${width}`);
      check((await page.locator('#comments').innerText()).includes('preserved the delivery handoff'), `Preservation history readable ${scheme}/${width}`);
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `Detail fits ${scheme}/${width}`);
      await page.screenshot({path:`/tmp/hb-handoff-qa/${scheme}-${width}-detail.png`,fullPage:true});
      await page.reload();
      await page.waitForFunction(() => model.detail?.issue.assignee === 'watcher:github');
      check(await page.locator('.state-pill.ready').innerText() === 'Ready', `Reload preserves Ready ${scheme}/${width}`);
    }
  }
  check(errors.length === 0, `No JavaScript errors: ${errors.join('; ')}`);
  return {passed:checks.length,checks};
}
