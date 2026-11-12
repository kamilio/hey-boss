// Run on a fresh project-settings web-fixture scenario.
async (page) => {
  let checks = 0;
  const check = (condition, message) => { if (!condition) throw Error(message); checks++; };
  const tab = name => page.getByRole('tab', {name, exact:true});
  const open = async () => {
    await page.locator('#project-more').click();
    await page.locator('#project-settings-trigger').click();
    await page.locator('#project-prs').waitFor({state:'visible'});
    await page.waitForFunction(() => !document.querySelector('#project-prs').disabled);
  };
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(page.url());
  await page.waitForFunction(() => model.project?.id === 'named:Settings fixture' && model.signature);
  const saved = [];
  let fail = null;
  await page.route('**/api/action', route => {
    const op = route.request().postDataJSON().operation;
    if (fail && (op.action === 'project_settings' || op.operation?.command === 'apply')) {
      return route.fulfill({json:{ok:false,error:{message:fail}}});
    }
    if (op.action === 'project_config' && op.operation.command === 'apply') saved.push(op);
    return route.continue();
  });
  await open();
  check(await page.locator('#project-settings-dialog').getByRole('tab').count() === 2, 'Configuration and activity');
  check(await page.locator('#project-settings-dialog').getByRole('tabpanel').count() === 1, 'Only one panel is exposed');
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
  check((await link.getAttribute('href')).includes('project=named%3ASettings+fixture&file=delivery-pr.md'), 'Workflow switch links directly to its project prompt');
  page.once('dialog',dialog=>dialog.dismiss());
  await link.click();
  check(await page.locator('#project-settings-dialog').isVisible(), 'Following a prompt link protects unsaved settings');
  await tab('Configuration').press('End');
  check(await tab('Activity').getAttribute('aria-selected') === 'true','End selects Activity');
  await tab('Activity').press('Home');
  check(await tab('Configuration').evaluate(el => el === document.activeElement), 'Home focuses first tab');
  await page.getByRole('button', {name:'Save',exact:true}).click();
  await page.locator('#project-settings-dialog').waitFor({state:'hidden'});
  check(await page.locator('#project-settings-dialog').evaluate(el => !el.open), 'Save closes dialog');
  check(saved.length === 1 && saved[0].operation.changes.prs_enabled && !('plan_template' in saved[0].operation.changes), 'Save includes all tab edits');
  check(!saved[0].operation.changes.drafts_enabled && !saved[0].operation.changes.artifacts_enabled, 'Save includes all resource switches');
  check(saved[0].operation.changes.paused && saved[0].operation.changes.pause_reason === 'Waiting for the next release', 'Pause and reason saved together');
  check(!('chief_enabled' in saved[0].operation.changes) && !('chief_prompt' in saved[0].operation.changes), 'Saving settings never restores retired configuration');
  await open();
  check(await page.locator('#project-paused').isChecked(), 'Pause persists on reopen');
  check(await page.locator('#project-pause-reason').inputValue() === 'Waiting for the next release', 'Pause reason persists on reopen');
  check(await page.locator('#project-prompt-layout, [data-reset-prompt="layout"]').count() === 0, 'Reopening does not recreate the sequence editor');
  for (const id of ['drafts', 'artifacts']) check(!await page.locator('#project-' + id).isChecked(), id + ' persists on reopen');
  await page.locator('#project-pause-reason').fill('');
  await tab('Activity').click();
  await page.getByRole('button', {name:'Save',exact:true}).click();
  check(await tab('Configuration').getAttribute('aria-selected') === 'true', 'Invalid hidden field reveals its tab');
  check(saved.length === 1, 'Invalid form does not save');
  await page.locator('#project-pause-reason').fill('Changed pause reason');
  fail = 'Fixture save failed';
  await page.getByRole('button', {name:'Save',exact:true}).click();
  await page.locator('#project-settings-error').waitFor();
  check(await page.locator('#project-settings-error').innerText() === 'Fixture save failed', 'Save error displayed');
  check(await page.locator('#project-pause-reason').inputValue() === 'Changed pause reason', 'Failed save preserves draft');
  await page.getByRole('button', {name:'Cancel',exact:true}).click();
  check(await page.locator('#project-more').evaluate(el => el === document.activeElement), 'Closing restores trigger focus');
  fail = null;
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
    await page.screenshot({path:`out/project-settings/configuration-${theme}-${width}.png`});
  }
  await page.getByRole('button', {name:'Cancel',exact:true}).click();
  fail = 'Fixture load failed';
  await page.locator('#project-more').click();
  await page.locator('#project-settings-trigger').click();
  await page.locator('#project-settings-error').waitFor();
  check(await page.locator('#project-settings-error').innerText() === 'Fixture load failed', 'Load error displayed');
  check(await page.getByRole('button', {name:'Save',exact:true}).isDisabled(), 'Failed load cannot save stale values');
  await page.getByRole('button', {name:'Cancel',exact:true}).click();
  check(errors.length === 0, `No browser exceptions: ${errors.join(', ')}`);
  console.log(`COMPLETE: ${checks}/${checks} settings checks`);
  return {passed: checks};
}
