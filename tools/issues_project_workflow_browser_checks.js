async page => {
  const checks = [], errors = [];
  const prefix = page.context().browser().browserType().name() === "webkit" ? "issue39-webkit" : "issue39";
  const check = (ok, label) => { if (!ok) throw Error(label); checks.push(label); };
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://127.0.0.1:4782/#project=named%3AWorkflow%20QA', {waitUntil:'domcontentloaded'});
  await page.reload({waitUntil:'domcontentloaded'});
  await page.waitForFunction(() => model.project?.id === 'named:Workflow QA');
  await page.evaluate(async () => api({action:'configure_project',prompt:'Claim and implement `{{issue_command}}`.',worktree_enabled:false,prs_enabled:false,prompt_overrides:{}}, model.project.id));
  await page.setViewportSize({width:1440,height:1000});
  const open = async () => {
    await page.getByRole('button', {name:'Project settings',exact:true}).click();
    await page.waitForFunction(() => !document.querySelector('#project-prompt').disabled && document.querySelector('#project-instructions-preview').getAttribute('aria-busy') === 'false');
  };
  const ready = async text => page.waitForFunction(text => {
    const preview=document.querySelector('#project-instructions-preview');
    return preview.getAttribute('aria-busy')==='false' && preview.textContent.includes(text);
  },text);
  const preview = () => page.locator('#project-instructions-preview').textContent();
  await open();
  check(await page.getByRole('heading',{name:'Project settings',exact:true}).isVisible(),'Settings title and entry point');
  check(!(await page.locator('#project-settings-dialog').innerText()).includes('commit_instruction'),'Retired tag removed from UI');
  check(await page.locator('#project-settings-form button[type=submit]').isDisabled(),'Save disabled for unchanged settings');
  await page.locator('#project-prompt').fill('/goal Implement {{issue_command}}.');
  await page.locator('#project-prompt-worktree').fill('Isolate {{number}} in a worktree.');
  await page.locator('#project-prompt-checkout').fill('Use the existing checkout for {{number}}.');
  await page.locator('#project-prompt-prs').fill('Publish a PR for {{number}}.');
  await page.locator('#project-prompt-main').fill('Publish main for {{number}}.');
  for (const worktree of [false,true]) for (const prs of [false,true]) {
    await page.locator('#project-worktree').setChecked(worktree);
    await page.locator('#project-prs').setChecked(prs);
    await ready(prs ? 'Publish a PR' : 'Publish main');
    const text=await preview();
    check(text.startsWith('Implement hey-boss issue view 1.'),`Shared instructions ${worktree}/${prs}`);
    check(text.includes(worktree?'Isolate 1':'Use the existing checkout for 1'),`Workspace branch ${worktree}/${prs}`);
    check(!text.includes(worktree?'Use the existing checkout':'Isolate'),`Inactive workspace excluded ${worktree}/${prs}`);
    check(!text.includes(prs?'Publish main':'Publish a PR'),`Inactive delivery excluded ${worktree}/${prs}`);
    check(await page.locator('.workflow-branch.active').count()===2,`Two included branches ${worktree}/${prs}`);
  }
  check(await page.locator('#project-goal-indicator').isVisible(),'Goal indicator');
  await page.locator('[data-reset-prompt=worktree]').click();
  await ready('dedicated Git worktree');
  check(await page.locator('#project-source-worktree').textContent()==='Built-in default','Reset restores default');
  check(!await page.locator('[data-reset-prompt=worktree]').isVisible(),'Reset hidden without override');
  await page.locator('#project-prompt-checkout').fill('Inactive edit must stay excluded.');
  await ready('dedicated Git worktree');
  check(!(await preview()).includes('Inactive edit'),'Inactive override excluded');
  await page.screenshot({path:`output/playwright/${prefix}-desktop-light.png`});
  check(await page.locator('.settings-preview').evaluate(el=>el.getBoundingClientRect().left>document.querySelector('.settings-editor').getBoundingClientRect().right),'Desktop preview on the right');
  const failedSave = async route => {
    if (route.request().postDataJSON()?.operation?.action === 'configure_project') {
      return route.fulfill({status:500,contentType:'application/json',body:JSON.stringify({ok:false,error:{code:'io_error',message:'Synthetic save failure'}})});
    }
    return route.continue();
  };
  await page.route('**/api/action', failedSave);
  await page.locator('#project-settings-form button[type=submit]').click();
  await page.getByRole('alert').filter({hasText:'Synthetic save failure'}).waitFor();
  check(await page.locator('#project-settings-dialog').evaluate(el=>el.open),'Failed save keeps editor open');
  check(await page.locator('#project-prompt').isEnabled(),'Failed save restores editable fields');
  check(await page.locator('#project-settings-form button[type=submit]').isEnabled(),'Failed save allows retry');
  await page.locator('#project-prompt-main').fill('Publish main for {{number}}.\n');
  await ready('Publish a PR');
  check(await page.getByRole('alert').filter({hasText:'Synthetic save failure'}).isVisible(),'Preview updates preserve save error');
  await page.unroute('**/api/action', failedSave);
  await page.locator('#project-settings-form button[type=submit]').click();
  await page.waitForFunction(()=>!document.querySelector('#project-settings-dialog').open);
  check(await page.locator('#project-settings-trigger').evaluate(el=>el===document.activeElement),'Save restores focus');
  await open();
  check(await page.locator('#project-worktree').isChecked() && await page.locator('#project-prs').isChecked(),'Choices persist');
  check(await page.locator('#project-prompt-checkout').inputValue()==='Inactive edit must stay excluded.','Inactive override persists');
  check(await page.locator('#project-prompt-worktree').inputValue()==='','Default reset persists');
  await page.emulateMedia({colorScheme:'dark'});
  await page.screenshot({path:`output/playwright/${prefix}-desktop-dark.png`});
  for (const width of [768,390,320]) {
    await page.setViewportSize({width,height:844});
    check(await page.locator('#project-settings-dialog').evaluate(el=>el.scrollWidth<=el.clientWidth && el.getBoundingClientRect().right<=innerWidth),`No modal overflow ${width}`);
    check(await page.locator('.project-settings-body').evaluate(el=>el.scrollWidth<=el.clientWidth),`No content overflow ${width}`);
    check(await page.locator('.settings-preview').evaluate(el=>el.getBoundingClientRect().top>=document.querySelector('.settings-editor').getBoundingClientRect().bottom),`Stacked preview ${width}`);
    await page.locator('#project-prompt').scrollIntoViewIfNeeded();
    await page.screenshot({path:`output/playwright/${prefix}-mobile-dark-${width}.png`});
    await page.locator('#project-instructions-preview').scrollIntoViewIfNeeded();
    await page.screenshot({path:`output/playwright/${prefix}-mobile-preview-${width}.png`});
    check(await page.getByRole('button',{name:'Save',exact:true}).isVisible(),`Sticky footer ${width}`);
  }
  await page.emulateMedia({colorScheme:'light'});
  await page.setViewportSize({width:390,height:844});
  await page.locator('#project-prompt').scrollIntoViewIfNeeded();
  await page.screenshot({path:`output/playwright/${prefix}-mobile-light.png`});
  await page.locator('#project-prompt').fill('Discard this edit');
  await page.getByRole('button',{name:'Cancel',exact:true}).click();
  await open();
  check(await page.locator('#project-prompt').inputValue()==='/goal Implement {{issue_command}}.','Cancel discards changes');
  await page.keyboard.press('Escape');
  check(!await page.locator('#project-settings-dialog').evaluate(el=>el.open),'Escape closes');
  check(await page.locator('#project-settings-trigger').evaluate(el=>el===document.activeElement),'Escape restores focus');
  await open();
  await page.keyboard.press('Tab');
  check(await page.evaluate(()=>document.activeElement.closest('#project-settings-dialog')!==null),'Keyboard focus contained');
  check(errors.length===0,`No JavaScript errors: ${errors.join(', ')}`);
  return {passed:checks.length,checks};
}
