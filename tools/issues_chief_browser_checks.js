async page => {
  const checks = [], errors = [], engine = page.context().browser().browserType().name();
  const check = (ok,label) => { if (!ok) throw Error(label); checks.push(label); };
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://127.0.0.1:4793/#project=named%3AChief%20QA',{waitUntil:'domcontentloaded'});
  await page.waitForFunction(() => model.project?.id === 'named:Chief QA');
  await page.evaluate(async () => api({action:'configure_project',chief_enabled:false,chief_prompt:(await api({action:'project_settings'},model.project.id)).chief_default_prompt},model.project.id));
  const open = async () => {
    await page.getByRole('button',{name:'Project settings',exact:true}).click();
    await page.waitForFunction(() => !document.querySelector('#project-chief').disabled);
  };
  const save = async () => {
    await page.getByRole('button',{name:'Save',exact:true}).click();
    await page.waitForFunction(() => !document.querySelector('#project-settings-dialog').open);
  };
  await open();
  check(!await page.getByRole('checkbox',{name:'Enable Chief'}).isChecked(),'Disabled by default');
  check(await page.getByRole('button',{name:'Save',exact:true}).isDisabled(),'Unchanged settings cannot save');
  check((await page.locator('#project-chief-help').textContent()).includes('uses no issue slots'),'Cadence, conversation reuse and independent capacity are explained');
  await page.locator('#project-chief-instructions summary').click();
  const defaultPrompt = await page.getByLabel('Organizing prompt').inputValue();
  check(defaultPrompt.includes('Workers handle code changes'),'Organizing default leaves execution to workers');
  await page.getByRole('checkbox',{name:'Enable Chief'}).check();
  await page.getByLabel('Organizing prompt').fill('Review open issues and PRs. Maintain a simple mindmap. Workers handle code changes.');
  check(await page.locator('#project-chief-state').textContent() === 'Enabled','Enabled state updates immediately');
  const failSave = route => route.fulfill({status:503,contentType:'application/json',body:JSON.stringify({ok:false,error:{message:'Synthetic Chief save failure'}})});
  await page.route('**/api/action',failSave);
  await page.getByRole('button',{name:'Save',exact:true}).click();
  await page.getByRole('alert').filter({hasText:'Synthetic Chief save failure'}).waitFor();
  check(await page.getByRole('checkbox',{name:'Enable Chief'}).isEnabled(),'Failed save restores editable fields');
  check(await page.locator('#project-chief-reset').isEnabled(),'Failed save restores reset button');
  await page.unroute('**/api/action',failSave);
  await save();
  check(await page.locator('#project-settings-trigger').evaluate(el => el === document.activeElement),'Save restores focus');
  await open();
  await page.locator('#project-chief-instructions summary').click();
  check(await page.getByRole('checkbox',{name:'Enable Chief'}).isChecked(),'Enable survives reload');
  check((await page.getByLabel('Organizing prompt').inputValue()).startsWith('Review open issues'),'Prompt persists separately');
  check(!(await page.locator('#project-instructions-preview').textContent()).includes('Review open issues'),'Chief prompt excluded from worker prompt');
  for (const theme of ['light','dark']) {
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:1000});
      await page.locator('.chief-settings').scrollIntoViewIfNeeded();
      check(await page.locator('#project-settings-dialog').evaluate(el => el.scrollWidth <= el.clientWidth && el.getBoundingClientRect().right <= innerWidth),`${theme} dialog fits ${width}`);
      check(await page.locator('.chief-settings').evaluate(el => el.scrollWidth <= el.clientWidth),`${theme} Chief fits ${width}`);
      check(await page.getByRole('button',{name:'Save',exact:true}).isVisible(),`${theme} footer reachable ${width}`);
      await page.screenshot({path:`output/playwright/issue43/${engine}-${theme}-${width}.png`});
    }
  }
  await page.locator('#project-chief-reset').click();
  check(await page.getByLabel('Organizing prompt').inputValue() === defaultPrompt,'Use default resets prompt');
  await page.getByLabel('Organizing prompt').fill('/goal Keep monitoring forever');
  await page.getByRole('button',{name:'Save',exact:true}).click();
  await page.getByRole('alert').filter({hasText:'cannot start with /goal'}).waitFor();
  check(await page.getByLabel('Organizing prompt').isEnabled(),'Invalid goal prompt rejected without losing edits');
  await page.locator('#project-chief-reset').click();
  await page.evaluate(async () => api({action:'configure_project',chief_prompt:'Changed elsewhere.'},model.project.id));
  await page.getByRole('button',{name:'Save',exact:true}).click();
  await page.getByRole('alert').filter({hasText:'changed elsewhere'}).waitFor();
  check(await page.getByRole('checkbox',{name:'Enable Chief'}).isChecked(),'Version conflict preserves local edits');
  await page.keyboard.press('Escape');
  check(await page.locator('#project-settings-trigger').evaluate(el => el === document.activeElement),'Escape restores focus');
  await page.evaluate(() => { location.hash = '#project=named%3AOther%20QA'; });
  await page.waitForFunction(() => model.project?.id === 'named:Other QA');
  await open();
  check(!await page.getByRole('checkbox',{name:'Enable Chief'}).isChecked(),'Other project remains disabled');
  await page.keyboard.press('Escape');
  check(errors.length === 0,'No JavaScript runtime errors: '+errors.join('; '));
  return {passed:checks.length,checks};
}
