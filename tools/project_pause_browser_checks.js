// Run with playwright-cli against an isolated Pause QA project on port 59643.
async page => {
  const errors = [], checks = [];
  const check = (ok, message) => { if (!ok) throw Error(message); checks.push(message); };
  page.on('pageerror', error => errors.push(error.message));
  const reason = 'Waiting for release <review> & approval';
  await page.goto('http://127.0.0.1:59643/#project=named%3APause%20QA');
  const open = async () => {
    await page.locator('#project-settings-trigger').click();
    await page.waitForFunction(() => !document.querySelector('#project-paused').disabled);
  };
  const save = async () => {
    await page.locator('#project-settings-form button[type=submit]').click();
    await page.locator('#project-settings-dialog').waitFor({state:'hidden'});
  };
  await open();
  check(!await page.locator('#project-paused').isChecked(), 'Existing project starts active');
  await page.locator('#project-paused').check();
  await page.locator('#project-settings-form button[type=submit]').click();
  check(await page.locator('#project-pause-reason').evaluate(el => el === document.activeElement), 'Pausing requires a reason');
  await page.locator('#project-pause-reason').fill(reason);
  await save();
  await page.locator('#paused-project-banner').waitFor();
  check(await page.locator('#paused-project-reason').innerText() === reason, 'Project banner shows reason as text');
  await page.reload();
  await page.locator('#paused-project-banner').waitFor();
  check(await page.locator('#paused-project-reason').innerText() === reason, 'Pause survives full reload');
  for (const theme of ['light', 'dark']) {
    await page.emulateMedia({colorScheme:theme});
    for (const width of [1280, 390, 320]) {
      await page.setViewportSize({width,height:844});
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), `Paused banner fits ${theme}/${width}`);
    }
  }
  await page.locator('#paused-project-settings').click();
  await page.waitForFunction(() => !document.querySelector('#project-paused').disabled);
  check(await page.locator('#project-paused').isChecked(), 'Banner opens saved pause setting');
  check(await page.locator('#project-pause-reason').inputValue() === reason, 'Settings retain exact reason');
  await page.locator('#project-paused').uncheck();
  await page.locator('#project-settings-cancel').click();
  check(await page.locator('#paused-project-banner').isVisible(), 'Cancel preserves paused state');
  await open();
  await page.locator('#project-paused').uncheck();
  await save();
  await page.locator('#paused-project-banner').waitFor({state:'hidden'});
  await page.reload();
  await open();
  check(!await page.locator('#project-paused').isChecked(), 'Resume persists after reload');
  check(!await page.locator('#project-pause-details').isVisible(), 'Resume clears pause details');
  await page.locator('#project-settings-cancel').click();
  check(errors.length === 0, 'No browser exceptions: ' + errors.join(', '));
  return {passed:checks.length, checks};
}
