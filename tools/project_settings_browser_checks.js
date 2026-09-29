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
  check(await page.getByRole('tab').count() === 5, 'Five settings tabs');
  check(await page.getByRole('tabpanel').count() === 1, 'Only one panel is exposed');
  check(await tab('Instructions').getAttribute('aria-selected') === 'true', 'Instructions selected initially');
  await page.locator('#project-prompt').fill('Edited implementation');
  await tab('Planning').click();
  await page.locator('#project-plan-template').fill('plans/edited-{number}.md');
  await page.locator('#project-prompt-plan').fill('Edited plan');
  await tab('Planning').press('ArrowRight');
  check(await tab('Workflow').evaluate(el => el === document.activeElement), 'Arrow selects and focuses next tab');
  await page.locator('#project-prs').check();
  await tab('Workflow').press('End');
  check(await tab('Preview').getAttribute('aria-selected') === 'true', 'End selects last tab');
  await page.waitForFunction(() => document.querySelector('#project-instructions-preview').textContent === 'Edited implementation');
  check(await page.locator('#project-preview-choices').innerText() === 'Existing checkout · Pull requests', 'Preview uses edits from other tabs');
  await tab('Preview').press('ArrowRight');
  check(await tab('Instructions').getAttribute('aria-selected') === 'true', 'Arrow wraps around');
  check(await page.locator('#project-prompt').inputValue() === 'Edited implementation', 'Draft survives tab switches');
  await tab('Instructions').press('ArrowLeft');
  await tab('Preview').press('Home');
  check(await tab('Instructions').evaluate(el => el === document.activeElement), 'Home focuses first tab');
  await page.getByRole('button', {name:'Save',exact:true}).click();
  check(await page.locator('#project-settings-dialog').evaluate(el => !el.open), 'Save closes dialog');
  check(await page.evaluate(() => saved.length === 1 && saved[0].prs_enabled && saved[0].prompt_overrides.plan === 'Edited plan' && saved[0].plan_template === 'plans/edited-{number}.md'), 'Save includes all tab edits');
  await open();
  await tab('Chief').click();
  await page.locator('#project-chief-instructions summary').click();
  await page.locator('#project-chief-prompt').fill('');
  await page.locator('#project-chief-instructions summary').click();
  await tab('Planning').click();
  await page.getByRole('button', {name:'Save',exact:true}).click();
  check(await tab('Chief').getAttribute('aria-selected') === 'true', 'Invalid hidden field reveals its tab');
  check(await page.locator('#project-chief-prompt').evaluate(el => el === document.activeElement && el.closest('details').open), 'Invalid field opens details and receives focus');
  check(await page.evaluate(() => saved.length === 1), 'Invalid form does not save');
  await tab('Instructions').click();
  await page.locator('#project-prompt').fill('');
  await tab('Preview').click();
  await page.getByRole('button', {name:'Save',exact:true}).click();
  check(await page.locator('#project-prompt').evaluate(el => el === document.activeElement), 'Multiple invalid tabs focus the first field');
  await page.locator('#project-prompt').fill('Edited implementation');
  await tab('Chief').click();
  await page.locator('#project-chief-prompt').fill('Edited chief');
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
  for (const width of [1280, 390, 320]) {
    await page.setViewportSize({width,height:800});
    for (const name of ['Instructions','Planning','Workflow','Chief','Preview']) {
      await tab(name).click();
      check(await page.locator('#project-settings-dialog').evaluate(el => el.scrollWidth <= el.clientWidth + 1), `${name} fits at ${width}px`);
      check(await page.getByRole('button', {name:'Save',exact:true}).evaluate(el => {const r=el.getBoundingClientRect();return r.bottom<=innerHeight && r.top>=0;}), `Save visible for ${name} at ${width}px`);
    }
    await tab('Workflow').click();
    await page.screenshot({path:`output/playwright/settings-tabs-${width}.png`});
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
