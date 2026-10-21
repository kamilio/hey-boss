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
  await page.evaluate(() => { window.fixtureSettings.prompt_overrides.layout = '{{delivery}}\n\n{{task}}'; });
  await open();
  check(await page.getByRole('tab').count() === 4, 'Configuration and three editor tabs');
  check(await page.getByRole('tabpanel').count() === 1, 'Only one panel is exposed');
  check(await tab('Configuration').getAttribute('aria-selected') === 'true', 'Configuration selected initially');
  check(!await page.locator('.settings-preview').isVisible(), 'Configuration has no prompt preview');
  check(!await page.locator('#project-paused').isChecked(), 'Projects start active');
  await page.locator('#project-paused').check();
  await page.locator('#project-pause-reason').fill('Waiting for the next release');
  for (const id of ['prs', 'worktree', 'drafts', 'artifacts', 'mindmaps']) {
    check(await page.locator('#project-' + id).isVisible(), id + ' is in Configuration');
  }
  await page.locator('#project-prs').check();
  await page.locator('#project-drafts').uncheck();
  await page.locator('#project-artifacts').uncheck();
  await page.locator('#project-mindmaps').uncheck();
  check(await page.locator('#project-plan-template').count() === 0, 'Terminal planning configuration is removed');
  await tab('Instructions').click();
  check(await page.locator('#project-prompt-layout').count() === 0, 'Prompt sequence editor is removed');
  await page.getByText('Task context sections', {exact:true}).click();
  await page.locator('#project-prompt-handoff').fill('Short handoff for {{number}}.');
  for (const key of ['subtask', 'github', 'dependencies', 'plan_document']) {
    check(await page.locator('#project-prompt-' + key).count() === 0, key + ' is fetched with the task, not an editor section');
  }
  await page.locator('#project-prompt').fill('Edited implementation');
  await tab('Planning').click();
  await page.locator('#project-prompt-plan').fill('Edited plan');
  await page.waitForFunction(() => document.querySelector('#project-instructions-preview').textContent === 'Edited plan');
  check(await page.locator('.settings-preview').isVisible(), 'Planning preview visible beside editor');
  await tab('Planning').press('ArrowRight');
  check(await tab('Workflow').evaluate(el => el === document.activeElement), 'Arrow selects and focuses next tab');
  check(!await page.locator('#project-prs').isVisible(), 'Workflow only contains prompt editors');
  await page.waitForFunction(() => document.querySelector('#project-instructions-preview').textContent === 'Edited implementation');
  check(await page.locator('#project-preview-choices').innerText() === 'Existing checkout · Pull requests', 'Workflow preview uses unsaved edits');
  await tab('Workflow').press('End');
  check(await tab('Workflow').getAttribute('aria-selected') === 'true', 'End selects last tab');
  check(await tab('Chief').count() === 0, 'Retired editor is absent');
  await tab('Workflow').press('ArrowRight');
  check(await tab('Configuration').getAttribute('aria-selected') === 'true', 'Arrow wraps around');
  await tab('Instructions').click();
  check(await page.locator('#project-prompt').inputValue() === 'Edited implementation', 'Draft survives tab switches');
  await tab('Instructions').press('ArrowLeft');
  await tab('Configuration').press('Home');
  check(await tab('Configuration').evaluate(el => el === document.activeElement), 'Home focuses first tab');
  await page.getByRole('button', {name:'Save',exact:true}).click();
  check(await page.locator('#project-settings-dialog').evaluate(el => !el.open), 'Save closes dialog');
  check(await page.evaluate(() => saved.length === 1 && saved[0].prs_enabled && saved[0].prompt_overrides.plan === 'Edited plan' && !('plan_template' in saved[0])), 'Save includes all tab edits');
  check(await page.evaluate(() => !saved[0].drafts_enabled && !saved[0].artifacts_enabled && !saved[0].mindmaps_enabled), 'Save includes all resource switches');
  check(await page.evaluate(() => saved[0].paused && saved[0].pause_reason === 'Waiting for the next release'), 'Pause and reason saved together');
  check(await page.evaluate(() => !('chief_enabled' in saved[0]) && !('chief_prompt' in saved[0])), 'Saving settings never restores retired configuration');
  check(await page.evaluate(() => saved[0].prompt_overrides.layout === '{{delivery}}\n\n{{task}}' && saved[0].prompt_overrides.handoff === 'Short handoff for {{number}}.'), 'Context edits preserve the saved sequence');
  await open();
  check(await page.locator('#project-paused').isChecked(), 'Pause persists on reopen');
  check(await page.locator('#project-pause-reason').inputValue() === 'Waiting for the next release', 'Pause reason persists on reopen');
  check(await page.locator('#project-prompt-layout, [data-reset-prompt="layout"]').count() === 0, 'Reopening does not recreate the sequence editor');
  for (const id of ['drafts', 'artifacts', 'mindmaps']) check(!await page.locator('#project-' + id).isChecked(), id + ' persists on reopen');
  await tab('Instructions').click();
  await page.locator('#project-prompt').fill('');
  await tab('Planning').click();
  await page.getByRole('button', {name:'Save',exact:true}).click();
  check(await tab('Instructions').getAttribute('aria-selected') === 'true', 'Invalid hidden field reveals its tab');
  check(await page.locator('#project-prompt').evaluate(el => el === document.activeElement), 'Invalid field receives focus');
  check(await page.evaluate(() => saved.length === 1), 'Invalid form does not save');
  await page.locator('#project-prompt').fill('Edited implementation again');
  await page.evaluate(() => { window.failSave = true; });
  await tab('Workflow').click();
  await page.getByRole('button', {name:'Save',exact:true}).click();
  check(await page.locator('#project-settings-error').innerText() === 'Fixture save failed', 'Save error stays visible across tabs');
  await tab('Instructions').click();
  check(await page.locator('#project-prompt').inputValue() === 'Edited implementation again', 'Failed save preserves draft');
  await page.getByRole('button', {name:'Cancel',exact:true}).click();
  check(await page.locator('#project-settings-trigger').evaluate(el => el === document.activeElement), 'Closing restores trigger focus');
  await page.evaluate(() => { window.failSave = false; });
  await open();
  check(await tab('Configuration').getAttribute('aria-selected') === 'true', 'Reopening starts at first tab');
  for (const theme of ['light','dark']) for (const width of [1280, 768, 390, 320]) {
    await page.emulateMedia({colorScheme:theme});
    await page.setViewportSize({width,height:800});
    for (const name of ['Configuration','Instructions','Planning','Workflow']) {
      await tab(name).click();
      check(await page.locator('.settings-preview').isVisible() === (name !== 'Configuration'), name + ' preview visibility at ' + width);
      if (width >= 800 && name !== 'Configuration') check(await page.locator('.settings-preview').evaluate(el => {
        const preview = el.getBoundingClientRect(), editor = document.querySelector('[role="tabpanel"]:not([hidden])').getBoundingClientRect();
        return preview.left >= editor.right && Math.abs(preview.top-editor.top)<2;
      }), name + ' editor left and preview right');
      check(await page.locator('#project-settings-dialog').evaluate(el => el.scrollWidth <= el.clientWidth + 1), `${name} fits at ${width}px`);
      check(await page.getByRole('button', {name:'Save',exact:true}).evaluate(el => {const r=el.getBoundingClientRect();return r.bottom<=innerHeight && r.top>=0;}), `Save visible for ${name} at ${width}px`);
    }
    await tab('Configuration').click();
    await page.screenshot({path:`/tmp/hey-boss-settings-qa/configuration-${theme}-${width}.png`});
    await tab('Workflow').click();
    await page.waitForFunction(() => document.querySelector('#project-instructions-preview').getAttribute('aria-busy') === 'false');
    check(await page.locator('#project-instructions-preview').textContent() === await page.locator('#project-prompt').inputValue(), 'Implementation preview preserves configured text');
    await page.screenshot({path:`/tmp/hey-boss-settings-qa/settings-${theme}-${width}.png`});
    if (width < 800) {
      await page.locator('.settings-preview').scrollIntoViewIfNeeded();
      await page.screenshot({path:`/tmp/hey-boss-settings-qa/preview-${theme}-${width}.png`});
    }
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
