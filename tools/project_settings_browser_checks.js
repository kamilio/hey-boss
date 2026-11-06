// Run with playwright-cli run-code --filename tools/project_settings_browser_checks.js.
async (page) => {
  let checks = 0;
  const check = (condition, message) => { if (!condition) throw Error(message); checks++; };
  const tab = name => page.getByRole('tab', {name, exact:true});
  const open = async () => {
    await page.getByRole('button', {name:'Project settings',exact:true}).click();
    await page.locator('#project-prs').waitFor({state:'visible'});
    await page.waitForFunction(() => !document.querySelector('#project-prs').disabled);
  };
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.goto(page.url().startsWith('http://127.0.0.1:') ? page.url().split('/').slice(0, 3).join('/') : 'http://127.0.0.1:59642');
  await open();
  check(await page.getByRole('tab').count() === 2, 'Configuration and activity');
  check(await page.getByRole('tabpanel').count() === 1, 'Only one panel is exposed');
  check(await tab('Configuration').getAttribute('aria-selected') === 'true', 'Configuration selected initially');
  check(!await page.locator('.settings-preview').isVisible(), 'Configuration has no prompt preview');
  check(!await page.locator('#project-paused').isChecked(), 'Projects start active');
  await page.locator('#project-paused').check();
  await page.locator('#project-pause-reason').fill('Waiting for the next release');
  for (const id of ['prs', 'worktree', 'drafts', 'artifacts']) {
    check(await page.locator('#project-' + id).isVisible(), id + ' is in Configuration');
  }
  await page.locator('#project-prs').check();
  await page.locator('#project-drafts').uncheck();
  await page.locator('#project-artifacts').uncheck();
  check(await page.locator('#project-plan-template').count() === 0, 'Terminal planning configuration is removed');
  const link = page.getByRole('link',{name:'delivery-pr.md',exact:true});
  check((await link.getAttribute('href')).includes('project=fixture&file=delivery-pr.md'), 'Workflow switch links directly to its project prompt');
  page.once('dialog',dialog=>dialog.dismiss());
  await link.click();
  check(await page.locator('#project-settings-dialog').isVisible(), 'Following a prompt link protects unsaved settings');
  await tab('Configuration').press('End');
  check(await tab('Activity').getAttribute('aria-selected') === 'true','End selects Activity');
  await tab('Activity').press('Home');
  check(await tab('Configuration').evaluate(el => el === document.activeElement), 'Home focuses first tab');
  await page.getByRole('button', {name:'Save',exact:true}).click();
  check(await page.locator('#project-settings-dialog').evaluate(el => !el.open), 'Save closes dialog');
  check(await page.evaluate(() => saved.length === 1 && saved[0].operation.changes.prs_enabled && !('plan_template' in saved[0].operation.changes)), 'Save includes all tab edits');
  check(await page.evaluate(() => !saved[0].operation.changes.drafts_enabled && !saved[0].operation.changes.artifacts_enabled), 'Save includes all resource switches');
  check(await page.evaluate(() => saved[0].operation.changes.paused && saved[0].operation.changes.pause_reason === 'Waiting for the next release'), 'Pause and reason saved together');
  check(await page.evaluate(() => !('chief_enabled' in saved[0].operation.changes) && !('chief_prompt' in saved[0].operation.changes)), 'Saving settings never restores retired configuration');
  await open();
  check(await page.locator('#project-paused').isChecked(), 'Pause persists on reopen');
  check(await page.locator('#project-pause-reason').inputValue() === 'Waiting for the next release', 'Pause reason persists on reopen');
  check(await page.locator('#project-prompt-layout, [data-reset-prompt="layout"]').count() === 0, 'Reopening does not recreate the sequence editor');
  for (const id of ['drafts', 'artifacts']) check(!await page.locator('#project-' + id).isChecked(), id + ' persists on reopen');
  await page.locator('#project-pause-reason').fill('');
  await tab('Activity').click();
  await page.getByRole('button', {name:'Save',exact:true}).click();
  check(await tab('Configuration').getAttribute('aria-selected') === 'true', 'Invalid hidden field reveals its tab');
  check(await page.evaluate(() => saved.length === 1), 'Invalid form does not save');
  await page.locator('#project-pause-reason').fill('Changed pause reason');
  await page.evaluate(() => { window.failSave = true; });
  await page.getByRole('button', {name:'Save',exact:true}).click();
  check(await page.locator('#project-settings-error').innerText() === 'Fixture save failed', 'Save error displayed');
  check(await page.locator('#project-pause-reason').inputValue() === 'Changed pause reason', 'Failed save preserves draft');
  await page.getByRole('button', {name:'Cancel',exact:true}).click();
  check(await page.locator('#project-settings-trigger').evaluate(el => el === document.activeElement), 'Closing restores trigger focus');
  await page.evaluate(() => { window.failSave = false; });
  await open();
  check(await tab('Configuration').getAttribute('aria-selected') === 'true', 'Reopening starts at first tab');
  for (const theme of ['light','dark']) for (const width of [1280, 768, 390, 320]) {
    await page.emulateMedia({colorScheme:theme});
    await page.setViewportSize({width,height:800});
    for (const name of ['Configuration','Activity']) {
      await tab(name).click();
      check(await page.locator('#project-settings-dialog').evaluate(el => el.scrollWidth <= el.clientWidth + 1), `${name} fits at ${width}px`);
      check(await page.getByRole('button', {name:'Save',exact:true}).evaluate(el => {const r=el.getBoundingClientRect();return r.bottom<=innerHeight && r.top>=0;}), `Save visible for ${name} at ${width}px`);
    }
    await tab('Configuration').click();
    await page.screenshot({path:`.config-qa/configuration-${theme}-${width}.png`});
  }
  await page.getByRole('button', {name:'Cancel',exact:true}).click();
  await page.evaluate(() => { window.failLoad = true; });
  await page.getByRole('button', {name:'Project settings',exact:true}).click();
  check(await page.locator('#project-settings-error').innerText() === 'Fixture load failed', 'Load error displayed');
  check(await page.getByRole('button', {name:'Save',exact:true}).isDisabled(), 'Failed load cannot save stale values');
  await page.getByRole('button', {name:'Cancel',exact:true}).click();
  check(errors.length === 0, `No browser exceptions: ${errors.join(', ')}`);
  console.log(`COMPLETE: ${checks}/${checks} settings checks`);
  return {passed: checks};
}
