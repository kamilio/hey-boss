// Run against tools/serve_project_settings_fixture.mjs with playwright-cli run-code.
async (page) => {
  let checks = 0;
  const check = (condition, message) => { if (!condition) throw Error(message); checks++; };
  const tab = name => page.getByRole('tab', {name, exact:true});
  const save = () => page.getByRole('button', {name:'Save', exact:true}).click();
  const cancel = () => page.getByRole('button', {name:'Cancel', exact:true}).click();
  const open = async () => {
    await page.getByRole('button', {name:'Project settings', exact:true}).click();
    await page.waitForFunction(() => document.querySelector('#project-instructions-preview').getAttribute('aria-busy') === 'false');
  };
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://127.0.0.1:59642');
  await page.evaluate(() => { window.fixtureSettings.prompt_overrides.layout = '{{delivery}}\n\n{{task}}'; });
  await open();
  check(await page.locator('#project-prompt-layout, #project-layout-prompts, [data-reset-prompt="layout"]').count() === 0, 'Sequence editor and reset are removed');
  check(!(await page.locator('#settings-panel-instructions').innerText()).includes('Prompt sequence'), 'No stale help for the removed editor');
  check(await page.locator('#settings-panel-instructions > :first-child h3').innerText() === 'Implementation', 'Instructions starts with task wording');
  check(await page.getByRole('button', {name:'Save', exact:true}).isDisabled(), 'Opening settings does not change instructions');
  check(await page.evaluate(() => preview.config.prompt_overrides.layout === fixtureSettings.prompt_overrides.layout), 'Preview retains saved sequence');
  await page.locator('#project-prompt').fill('Edited implementation');
  await page.getByText('Task context sections', {exact:true}).click();
  await page.locator('#project-prompt-handoff').fill('Fixture handoff');
  await tab('Instructions').press('ArrowRight');
  check(await tab('Planning').evaluate(el => el === document.activeElement), 'Keyboard selects and focuses next tab');
  await page.locator('#project-prompt-plan').fill('Edited plan');
  await page.waitForFunction(() => document.querySelector('#project-instructions-preview').textContent === 'Edited plan');
  await tab('Workflow').click();
  await page.locator('#project-prs').check();
  await page.waitForFunction(() => document.querySelector('#project-instructions-preview').textContent === 'Edited implementation');
  await save();
  check(await page.locator('#project-settings-dialog').evaluate(el => !el.open), 'Save closes settings');
  check(await page.evaluate(() => saved[0].prompt_overrides.layout === '{{delivery}}\n\n{{task}}' && saved[0].prompt_overrides.handoff === 'Fixture handoff' && saved[0].prompt_overrides.plan === 'Edited plan' && saved[0].prs_enabled), 'All tab edits save without erasing the sequence');
  await open();
  check(await page.locator('#project-prompt-layout').count() === 0, 'Reopening does not recreate the editor');
  await page.locator('#project-prompt').fill('');
  await tab('Workflow').click();
  await save();
  check(await tab('Instructions').getAttribute('aria-selected') === 'true' && await page.locator('#project-prompt').evaluate(el => el === document.activeElement), 'Invalid field reveals its tab and receives focus');
  check(await page.evaluate(() => saved.length === 1), 'Invalid form does not save');
  await page.locator('#project-prompt').fill('Changed again');
  await page.evaluate(() => { window.failSave = true; });
  await save();
  check(await page.locator('#project-settings-error').innerText() === 'Fixture save failed', 'Save failure is visible');
  check(await page.locator('#project-prompt').inputValue() === 'Changed again', 'Failed save preserves draft');
  await cancel();
  check(await page.locator('#project-settings-trigger').evaluate(el => el === document.activeElement), 'Closing restores focus');
  await page.evaluate(() => { window.failSave = false; });
  await open();
  // Start each visual review with the same collapsed context sections as a new visit.
  await page.locator('#settings-panel-instructions details').evaluate(el => { el.open = false; });
  for (const theme of ['light', 'dark']) for (const width of [1280, 768, 390, 320]) {
    await page.emulateMedia({colorScheme:theme});
    await page.setViewportSize({width, height:800});
    for (const name of ['Instructions', 'Planning', 'Workflow']) {
      await tab(name).click();
      check(await page.locator('#project-settings-dialog').evaluate(el => el.scrollWidth <= el.clientWidth + 1), `${name} fits at ${width}px`);
      check(await page.getByRole('button', {name:'Save', exact:true}).evaluate(el => { const r = el.getBoundingClientRect(); return r.bottom <= innerHeight && r.top >= 0; }), `Save visible for ${name} at ${width}px`);
      if (width > 800) check(await page.locator('.settings-preview').evaluate(el => {
        const p = el.getBoundingClientRect(), e = document.querySelector('[role="tabpanel"]:not([hidden])').getBoundingClientRect();
        return p.left >= e.right && Math.abs(p.top - e.top) < 2;
      }), 'Preview aligns with editor');
    }
    await tab('Instructions').click();
    check(await page.locator('#project-prompt').evaluate(el => {
      const f = el.getBoundingClientRect(), b = document.querySelector('.project-settings-body').getBoundingClientRect();
      return f.top >= b.top && f.top < b.bottom;
    }), `Task wording immediately visible at ${width}px`);
    await page.screenshot({path:`output/playwright/prompt-sequence-${theme}-${width}.png`});
  }
  await cancel();
  await page.evaluate(() => { delete window.fixtureSettings.prompt_overrides.layout; });
  await open();
  await tab('Planning').click();
  await page.locator('#project-drafts').uncheck();
  await save();
  check(await page.evaluate(() => saved.at(-1).prompt_overrides.layout == null), 'A project without an override does not inherit the previous sequence');
  await page.evaluate(() => { window.failLoad = true; });
  await page.getByRole('button', {name:'Project settings', exact:true}).click();
  check(await page.locator('#project-settings-error').innerText() === 'Fixture load failed', 'Load failure is visible');
  check(await page.getByRole('button', {name:'Save', exact:true}).isDisabled(), 'Failed load cannot save stale values');
  await cancel();
  check(errors.length === 0, `No browser exceptions: ${errors.join(', ')}`);
  return {passed:checks};
}
