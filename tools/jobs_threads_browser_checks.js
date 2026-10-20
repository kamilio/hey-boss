// Run against serve_jobs_ui_fixture.mjs; mutations stay in its disposable store.
async (page) => {
  const origin = 'http://127.0.0.1:59649';
  const check = (ok, name) => { if (!ok) throw Error(name); };
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/api/jobs/runtime', route => route.fulfill({json: {
    ok: true, machine: 'fixture', catalog: {models: []}, machines: []
  }}));
  await page.goto(origin + '/jobs#project=' + encodeURIComponent('named:Jobs UI QA'));
  await page.locator('#job-new').click();
  check(await page.locator('#job-thread-mode').inputValue() === 'new', 'New jobs start fresh');
  await page.locator('#job-name').fill('Thread mode check');
  await page.locator('#job-markdown').fill(await (await page.request.get('http://127.0.0.1:52049/fixture-instructions.md')).text());
  await page.locator('#job-model').fill('fixture-model');
  await page.locator('#job-enabled').uncheck();
  await page.getByRole('button', {name: 'Create job', exact: true}).click();
  await page.waitForSelector('#job-run');
  await page.reload();
  await page.waitForSelector('#job-thread-mode');
  check(await page.locator('#job-thread-mode').inputValue() === 'new', 'Saved default remains fresh');
  await page.locator('#job-thread-mode').selectOption('resume');
  await page.getByRole('button', {name: 'Save changes', exact: true}).click();
  await page.waitForFunction(() => document.querySelector('.job-panel-subtitle')?.textContent.includes('Revision 2'));
  await page.reload();
  await page.waitForSelector('#job-thread-mode');
  check(await page.locator('#job-thread-mode').inputValue() === 'resume', 'Explicit resume persists');
  for (const width of [1280, 390]) {
    await page.setViewportSize({width, height: 900});
    await page.locator('#job-thread-mode').scrollIntoViewIfNeeded();
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'No horizontal overflow');
    await page.screenshot({path: '/tmp/hey-boss-job-thread-' + width + '.png'});
  }
  await page.locator('#job-thread-mode').selectOption('new');
  await page.getByRole('button', {name: 'Save changes', exact: true}).click();
  await page.waitForFunction(() => document.querySelector('.job-panel-subtitle')?.textContent.includes('Revision 3'));
  await page.reload();
  await page.waitForSelector('#job-thread-mode');
  check(await page.locator('#job-thread-mode').inputValue() === 'new', 'Returning to fresh threads persists');
  check(errors.length === 0, errors.join('\n'));
  return {ok: true, checks: 'New default, save/reload, explicit resume, return to new, desktop/mobile layout'};
}
