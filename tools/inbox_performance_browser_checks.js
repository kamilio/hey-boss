// Use the native Inbox fixture with 8,000 archived notices added to its isolated store.
async page => {
  const checks = [], errors = [], requests = [];
  const check = (ok, label) => { if (!ok) throw Error(label); checks.push(label); };
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => {
    if (request.url().endsWith('/api/inbox')) requests.push(request.postDataJSON().action);
  });
  const origin = await page.evaluate(() => location.origin);
  await page.goto(origin + '/#view=inbox');
  await page.reload();
  await page.waitForSelector('.notice-row');
  const cold = await page.evaluate(() => ({
    tasks: inboxTasks.length, archived: inboxArchived,
    request: performance.getEntriesByType('resource').find(entry => entry.name.endsWith('/api/inbox'))?.toJSON(),
  }));
  check(cold.tasks === 5 && cold.archived === 8001, 'Unread excludes all 8,001 archived notices');
  check(!requests.includes('list'), 'Opening unread never requests full history');
  check(cold.request.decodedBodySize < 16000, 'Unread payload stays small with a large history');
  check(await page.locator('#inbox-project-filter option').allTextContents().then(items => items.includes('History only')), 'Archived-only project remains in the filter');
  for (const scheme of ['light', 'dark']) {
    await page.emulateMedia({colorScheme: scheme});
    for (const width of [1440, 390]) {
      await page.setViewportSize({width, height: 980});
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${scheme} ${width}px fits without overflow`);
      await page.screenshot({path: `output/playwright/inbox-scale-${scheme}-${width}.png`, fullPage: true});
    }
  }
  await page.getByRole('tab', {name: 'Activity 8001', exact: true}).click();
  await page.waitForFunction(() => inboxScope === 'list');
  check(await page.locator('.notice-row').count() === 1000, 'Activity retains pagination');
  await page.getByRole('searchbox', {name: 'Search Inbox'}).fill('Archived release report 8000');
  await page.waitForFunction(() => document.querySelectorAll('.notice-row').length === 1);
  check(await page.locator('.notice-title').innerText() === 'Archived release report 8000', 'Full Activity history remains searchable');
  check(await page.locator('#inbox-search').evaluate(input => input === document.activeElement), 'Warm filtering retains keyboard focus');
  await page.screenshot({path: 'output/playwright/inbox-scale-activity.png', fullPage: true});
  await page.getByRole('searchbox', {name: 'Search Inbox'}).fill('');
  await page.waitForFunction(() => model.route.inbox_search === '');
  await page.getByRole('tab', {name: 'Unread 5', exact: true}).click();
  await page.waitForFunction(() => inboxScope === 'unread');
  const warm = await page.evaluate(async () => {
    inboxAt = Date.now() - 5000;
    const start = performance.now();
    const loading = renderInboxRoute(model.sequence);
    const visible = !!document.querySelector('.notice-row');
    const elapsed = performance.now() - start;
    await loading;
    return {visible, elapsed};
  });
  check(warm.visible, 'Stale unread rows render synchronously during revalidation');
  await page.getByRole('combobox', {name: 'Filter Inbox by project'}).selectOption('History only');
  await page.waitForSelector('[data-inbox-clear]');
  check(await page.locator('#notice-list').innerText().then(text => text.includes('No matching notices')), 'Archived-only filter shows the correct unread empty state');
  await page.screenshot({path: 'output/playwright/inbox-scale-empty.png', fullPage: true});
  check(errors.length === 0, 'No JavaScript errors');
  return {checks, cold, warm};
}
