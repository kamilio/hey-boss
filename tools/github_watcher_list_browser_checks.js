// Run with playwright-cli run-code --filename against serve_assignment_watch_fixture.mjs.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  const origin = await page.evaluate(() => location.origin);
  await page.goto(origin + '/#project=named%3AAssignment%20QA&view=issues&state=open&owner=all');
  await page.reload();
  await page.waitForFunction(() => model.signature);
  const opener = number => page.getByRole('button', {name:`Open GitHub watcher for issue #${number}`,exact:true});
  check(await opener(1).count() === 1, 'Waiting watcher has a list opener');
  const url = page.url();
  await opener(7).click();
  const dialog = page.getByRole('dialog', {name:'GitHub watcher',exact:true});
  await dialog.getByText('GitHub rate limit reached', {exact:true}).waitFor();
  check(page.url() === url, 'Opening status preserves list filters and URL');
  check(await dialog.getByText(/Last fetch/).count() === 1, 'Last fetch is visible');
  check(await dialog.getByText(/Retry after/).count() === 1, 'Retry schedule is visible');
  const refreshRequest = page.waitForRequest(request => request.url().endsWith('/api/action') && request.postDataJSON()?.operation?.action === 'refresh_github');
  await dialog.getByRole('button', {name:'Fetch now'}).click();
  const request = (await refreshRequest).postDataJSON();
  check(request.project === 'named:Assignment QA' && request.operation.number === 7, 'Fetch now targets the opened issue');
  await dialog.getByText('Fetch queued', {exact:true}).waitFor();
  check(await dialog.getByRole('button', {name:'Fetch now'}).isDisabled(), 'Queued fetch cannot be submitted twice');
  await page.keyboard.press('Escape');
  check(!await dialog.isVisible(), 'Escape closes watcher');
  check(await opener(7).evaluate(el => el === document.activeElement), 'Closing restores opener focus');
  await opener(3).click();
  await dialog.getByText(/is working on Devbox/).waitFor();
  check((await dialog.getByRole('link', {name:'Open agent conversation'}).getAttribute('href')).includes('agent=codex%3Afixture'), 'Active watcher exposes agent conversation');
  await dialog.getByText('Required checks', {exact:true}).click();
  check(await dialog.getByText('Unit tests', {exact:true}).isVisible(), 'Required checks can be inspected');
  const nextPoll = page.waitForRequest(r => r.url().endsWith('/api/action') && r.postDataJSON()?.operation?.action === 'view' && r.postDataJSON()?.operation?.number === 3);
  await nextPoll;
  check(await dialog.getByText('Unit tests', {exact:true}).isVisible(), 'Polling preserves expanded checks');
  await page.keyboard.press('Escape');
  for (const scheme of ['light','dark']) {
    await page.emulateMedia({colorScheme:scheme});
    for (const width of [1440,390,320]) {
      await page.setViewportSize({width,height:800});
      await opener(6).click();
      await dialog.getByText('Recent review feedback').waitFor();
      await dialog.getByText('Recent review feedback').click();
      check(await dialog.getByText(/Please handle a connection drop/).isVisible(), `Review feedback visible ${scheme}/${width}`);
      check(await dialog.evaluate(el => { const r=el.getBoundingClientRect(); return r.left>=0 && r.right<=innerWidth && r.top>=0 && r.bottom<=innerHeight && el.scrollWidth<=el.clientWidth; }), `Dialog fits ${scheme}/${width}`);
      if (width === 390) await page.screenshot({path:`output/playwright/assignment-watch/watcher-${scheme}.png`});
      await dialog.getByRole('button',{name:'Close watcher'}).click();
    }
  }
  check(errors.length === 0, 'No browser runtime errors: ' + errors.join(', '));
  return {checks:checks.length,passed:checks};
}
