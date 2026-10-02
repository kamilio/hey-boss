// Run with playwright-cli run-code --filename tools/project_settings_browser_checks.js.
async (page) => {
  let checks = 0;
  const check = (condition, message) => { if (!condition) throw Error(message); checks++; };
  const tab = name => page.getByRole('tab', {name, exact:true});
  const open = async () => {
    await page.getByRole('button', {name:'Project settings',exact:true}).click();
    await page.locator('#project-prompt').waitFor({state:'visible'});
  };
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.goto('http://127.0.0.1:59642');
  await open();
  check(await page.getByRole('tab').count() === 4, 'Four editor tabs');
  check(await page.getByRole('tabpanel').count() === 1, 'Only one panel is exposed');
  check(await tab('Instructions').getAttribute('aria-selected') === 'true', 'Instructions selected initially');
  check(await page.locator('#project-prompt-layout').isVisible(), 'Prompt sequence is visible before task wording');
  await page.locator('#project-prompt-layout').fill('{{delivery}}\n\n{{task}}');
  await page.getByText('Task context sections', {exact:true}).click();
  await page.locator('#project-prompt-handoff').fill('Short handoff for {{number}}.');
  for (const key of ['subtask', 'github', 'dependencies', 'plan_document']) {
    check(await page.locator('#project-prompt-' + key).count() === 0, key + ' is fetched with the task, not an editor section');
  }
  await page.locator('#project-prompt').fill('Edited implementation');
  await tab('Planning').click();
  await page.locator('#project-plan-template').fill('plans/edited-{number}.md');
  await page.locator('#project-prompt-plan').fill('Edited plan');
  await page.waitForFunction(() => document.querySelector('#project-instructions-preview').textContent === 'Edited plan');
  check(await page.locator('.settings-preview').isVisible(), 'Planning preview visible beside editor');
  await tab('Planning').press('ArrowRight');
  check(await tab('Workflow').evaluate(el => el === document.activeElement), 'Arrow selects and focuses next tab');
  await page.locator('#project-prs').check();
  await page.waitForFunction(() => document.querySelector('#project-instructions-preview').textContent === 'Edited implementation');
  check(await page.locator('#project-preview-choices').innerText() === 'Existing checkout · Pull requests', 'Workflow preview uses unsaved edits');
  await tab('Workflow').press('End');
  check(await tab('Chief').getAttribute('aria-selected') === 'true', 'End selects last tab');
  await page.waitForFunction(() => document.querySelector('#project-instructions-preview').textContent === 'Fixture chief');
  check(await page.locator('#project-prompt-chief_wrapper').count() === 0, 'Chief has no wrapper editor');
  check(await page.evaluate(() => !!window.fixtureSettings.prompt_overrides.chief_wrapper), 'Legacy saved wrapper is present but ignored by the preview');
  if (!await page.locator('#project-chief-prompt').isVisible()) await page.locator('#project-chief-instructions summary').click();
  await page.locator('#project-chief-prompt').fill('Fixture chief {{project}}');
  await page.waitForFunction(() => document.querySelector('#project-instructions-preview').textContent === 'Fixture chief {{project}}');
  check(true, 'Chief preview preserves literal template syntax');
  check(await page.locator('.settings-preview').isVisible(), 'Chief preview visible');
  await tab('Chief').press('ArrowRight');
  check(await tab('Instructions').getAttribute('aria-selected') === 'true', 'Arrow wraps around');
  check(await page.locator('#project-prompt').inputValue() === 'Edited implementation', 'Draft survives tab switches');
  await tab('Instructions').press('ArrowLeft');
  await tab('Chief').press('Home');
  check(await tab('Instructions').evaluate(el => el === document.activeElement), 'Home focuses first tab');
  await page.getByRole('button', {name:'Save',exact:true}).click();
  check(await page.locator('#project-settings-dialog').evaluate(el => !el.open), 'Save closes dialog');
  check(await page.evaluate(() => saved.length === 1 && saved[0].prs_enabled && saved[0].prompt_overrides.plan === 'Edited plan' && saved[0].plan_template === 'plans/edited-{number}.md'), 'Save includes all tab edits');
  check(await page.evaluate(() => saved[0].prompt_overrides.layout === '{{delivery}}\n\n{{task}}' && saved[0].prompt_overrides.handoff === 'Short handoff for {{number}}.'), 'Sequence and context overrides save together');
  await open();
  check(await page.locator('#project-prompt-layout').inputValue() === '{{delivery}}\n\n{{task}}', 'Sequence survives reopening');
  await page.locator('[data-reset-prompt="layout"]').click();
  check(await page.locator('#project-prompt-layout').inputValue() === '' && (await page.locator('#project-prompt-layout').getAttribute('placeholder')).includes('{{workspace}}'), 'Reset restores the default sequence');
  await tab('Chief').click();
  await page.locator('#project-chief-prompt').fill('');
  await page.locator('#project-chief-instructions summary').click();
  await tab('Planning').click();
  await page.getByRole('button', {name:'Save',exact:true}).click();
  check(await tab('Chief').getAttribute('aria-selected') === 'true', 'Invalid hidden field reveals its tab');
  check(await page.locator('#project-chief-prompt').evaluate(el => el === document.activeElement && el.closest('details').open), 'Invalid field opens details and receives focus');
  check(await page.evaluate(() => saved.length === 1), 'Invalid form does not save');
  await tab('Instructions').click();
  await page.locator('#project-prompt').fill('');
  await tab('Chief').click();
  await page.getByRole('button', {name:'Save',exact:true}).click();
  check(await page.locator('#project-prompt').evaluate(el => el === document.activeElement), 'Multiple invalid tabs focus the first field');
  await page.locator('#project-prompt').fill('Edited implementation');
  await tab('Chief').click();
  await page.locator('#project-chief-prompt').fill('Edited chief');
  await page.waitForFunction(() => document.querySelector('#project-instructions-preview').textContent === 'Edited chief');
  await page.evaluate(() => { window.failSave = true; });
  await tab('Workflow').click();
  await page.getByRole('button', {name:'Save',exact:true}).click();
  check(await page.locator('#project-settings-error').innerText() === 'Fixture save failed', 'Save error stays visible across tabs');
  await tab('Chief').click();
  check(await page.locator('#project-chief-prompt').inputValue() === 'Edited chief', 'Failed save preserves draft');
  await page.getByRole('button', {name:'Cancel',exact:true}).click();
  check(await page.locator('#project-settings-trigger').evaluate(el => el === document.activeElement), 'Closing restores trigger focus');
  await page.evaluate(() => { window.failSave = false; });
  await open();
  check(await tab('Instructions').getAttribute('aria-selected') === 'true', 'Reopening starts at first tab');
  for (const theme of ['light','dark']) for (const width of [1280, 768, 390, 320]) {
    await page.emulateMedia({colorScheme:theme});
    await page.setViewportSize({width,height:800});
    for (const name of ['Instructions','Planning','Workflow','Chief']) {
      await tab(name).click();
      check(await page.locator('.settings-preview').isVisible(), name + ' retains preview at ' + width);
      if (width >= 800) check(await page.locator('.settings-preview').evaluate(el => {
        const preview = el.getBoundingClientRect(), editor = document.querySelector('[role="tabpanel"]:not([hidden])').getBoundingClientRect();
        return preview.left >= editor.right && Math.abs(preview.top-editor.top)<2;
      }), name + ' editor left and preview right');
      check(await page.locator('#project-settings-dialog').evaluate(el => el.scrollWidth <= el.clientWidth + 1), `${name} fits at ${width}px`);
      check(await page.getByRole('button', {name:'Save',exact:true}).evaluate(el => {const r=el.getBoundingClientRect();return r.bottom<=innerHeight && r.top>=0;}), `Save visible for ${name} at ${width}px`);
    }
    await tab('Chief').click();
    await page.waitForFunction(() => document.querySelector('#project-instructions-preview').getAttribute('aria-busy') === 'false');
    check(await page.locator('#project-instructions-preview').textContent() === await page.locator('#project-chief-prompt').inputValue(), 'Chief preview equals configured prompt');
    await page.screenshot({path:`output/playwright/chief-settings-${theme}-${width}.png`});
    if (width < 800) {
      await page.locator('.settings-preview').scrollIntoViewIfNeeded();
      await page.screenshot({path:`output/playwright/chief-preview-${theme}-${width}.png`});
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
