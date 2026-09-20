// Run through playwright-cli against an isolated issue server on port 4794.
async page => {
  // The shared development machine may be compiling other projects concurrently.
  page.setDefaultTimeout(90000);
  page.setDefaultNavigationTimeout(90000);
  const checks = [], errors = [];
  page.on('dialog', async dialog => {
    if (dialog.type() !== 'beforeunload') throw Error(`Unexpected dialog: ${dialog.type()}`);
    await dialog.accept();
  });
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://127.0.0.1:4794/#project=named%3ADraft%20QA', {waitUntil:'domcontentloaded'});
  await page.waitForFunction(() => model.project?.id === 'named:Draft QA');
  await page.evaluate(() => {
    for (const key of Object.keys(localStorage)) if (key.startsWith('hey-boss:editor:')) localStorage.removeItem(key);
  });
  const action = operation => page.evaluate(operation => api(operation, model.project.id), operation);
  await action({action:'configure_project',drafts_enabled:true});
  const created = await action({action:'create',title:'Plan a calmer onboarding flow',body:'## A thoughtful first visit\n\nMake the next step clear.\n\n- [ ] Review the empty state\n- [ ] Test on a narrow screen',labels:['design'],draft:true});
  const number = created.issue.number;
  await page.reload({waitUntil:'domcontentloaded'});
  const row = page.locator(`.issue-row[data-issue-number="${number}"]`);
  await row.waitFor();
  check(await row.locator('.draft-badge').isVisible(), 'Draft has a distinct list badge');
  await row.locator('.issue-title').click();
  await page.locator('.draft-notice').waitFor();
  check(await page.locator('.state-pill.draft').innerText() === 'Draft', 'Detail status identifies a draft');
  check((await page.locator('.draft-notice').innerText()).includes('Agents won’t pick up'), 'Draft explains why agents skip it');
  await page.locator('#comment-body').fill('Keep this unsent comment through readiness changes.');
  const fail = async route => route.request().postDataJSON()?.operation?.action === 'undraft'
    ? route.fulfill({status:500,contentType:'application/json',body:JSON.stringify({ok:false,error:{code:'io_error',message:'Synthetic plan sync failure'}})}) : route.continue();
  await page.route('**/api/action', fail);
  await page.getByRole('button', {name:'Mark ready',exact:true}).click();
  await page.locator('#draft-error').filter({hasText:'Synthetic plan sync failure'}).waitFor();
  check(await page.getByRole('button',{name:'Mark ready',exact:true}).isEnabled(), 'Readiness failures allow retry');
  check(await page.locator('.state-pill.draft').isVisible(), 'Failed readiness keeps draft status');
  check((await page.locator('#comment-body').inputValue()).startsWith('Keep this'), 'Failed readiness preserves comment');
  await page.unroute('**/api/action', fail);
  await page.getByRole('button', {name:'Mark ready',exact:true}).click();
  await page.locator('[data-draft-action="draft"]').waitFor();
  check(await page.locator('.draft-notice').count() === 0, 'Ready issue removes draft notice');
  check((await page.locator('#comment-body').inputValue()).startsWith('Keep this'), 'Successful readiness preserves comment');
  check(await page.locator('[data-draft-action="draft"]').evaluate(el => el === document.activeElement), 'Readiness restores keyboard focus');
  await page.getByRole('button',{name:'Move to draft',exact:true}).click();
  await page.locator('.draft-notice').waitFor();
  await page.getByRole('button',{name:'Edit',exact:true}).click();
  await page.waitForFunction(() => !document.querySelector('#editor-draft').disabled);
  check(await page.locator('#editor-draft').isChecked(), 'Editing preserves draft status');
  check(await page.locator('#editor-submit').innerText() === 'Save draft', 'Editing draft has a clear save label');
  await page.locator('#editor-draft').uncheck();
  check(await page.locator('#editor-submit').innerText() === 'Save & mark ready', 'Editor makes readiness transition explicit');
  await page.keyboard.press('Escape');
  await page.getByRole('button',{name:'All issues',exact:true}).click();
  await row.waitFor();
  await page.locator('#new-issue').click();
  await page.waitForFunction(() => !document.querySelector('#editor-draft').disabled);
  check(!await page.locator('#editor-draft').isChecked(), 'New issues default to ready');
  await page.locator('#editor-subject').fill('A saved draft from the editor');
  await page.locator('#editor-draft').focus();
  await page.keyboard.press('Space');
  check(await page.locator('#editor-submit').innerText() === 'Create draft', 'Keyboard draft choice updates creation label');
  await page.keyboard.press('Escape');
  await page.locator('#new-issue').click();
  check(await page.locator('#editor-draft').isChecked(), 'Unsent draft choice is restored');
  await page.locator('#editor-submit').click();
  await page.waitForFunction(() => !document.querySelector('#editor-dialog').open);
  const savedRow = page.locator('.issue-row').filter({has:page.getByRole('link',{name:'A saved draft from the editor',exact:true})});
  await savedRow.locator('.draft-badge').waitFor();
  check(true, 'Draft creation persists and returns to list');
  await page.locator('#new-issue').click();
  await page.waitForFunction(() => !document.querySelector('#editor-draft').disabled);
  check(!await page.locator('#editor-draft').isChecked(), 'Next issue returns to ready default');
  await page.keyboard.press('Escape');
  await action({action:'configure_project',drafts_enabled:false});
  await page.locator('#new-issue').click();
  await page.waitForFunction(() => document.querySelector('#editor-draft-help').textContent.includes('disabled'));
  check(await page.locator('#editor-draft').isDisabled(), 'Disabled project drafts cannot be selected');
  // Generic busy-state restoration must not enable a forbidden draft choice.
  await page.evaluate(() => { editorBusy(true); editorBusy(false); });
  check(await page.locator('#editor-draft').isDisabled(), 'Save-state restoration preserves draft eligibility');
  await page.keyboard.press('Escape');
  await row.locator('.issue-title').click();
  await page.locator('.draft-notice').waitFor();
  check(await page.getByRole('button',{name:'Mark ready',exact:true}).isEnabled(), 'Existing drafts can be marked ready with drafts disabled');
  await page.getByRole('button',{name:'Mark ready',exact:true}).click();
  await page.locator('[data-draft-action="draft"]').waitFor();
  check(await page.getByRole('button',{name:'Move to draft',exact:true}).isDisabled(), 'Ready detail explains disabled drafting');
  await action({action:'configure_project',drafts_enabled:true});
  await page.reload({waitUntil:'domcontentloaded'});
  await page.getByRole('button',{name:'Move to draft',exact:true}).click();
  await page.locator('.draft-notice').waitFor();
  const prefix = `output/playwright/draft-${page.context().browser().browserType().name()}`;
  for (const scheme of ['light','dark']) {
    await page.emulateMedia({colorScheme:scheme});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:900});
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `Detail bounds ${scheme}/${width}`);
      await page.screenshot({path:`${prefix}-detail-${scheme}-${width}.png`,fullPage:true});
    }
    await page.setViewportSize({width:1440,height:1000});
    await page.getByRole('button',{name:'All issues',exact:true}).click();
    await row.waitFor();
    await page.screenshot({path:`${prefix}-list-${scheme}.png`});
    await page.locator('#new-issue').click();
    await page.waitForFunction(() => !document.querySelector('#editor-draft').disabled);
    await page.locator('#editor-subject').fill('Review the draft experience');
    await page.locator('#editor-draft').check();
    for (const width of [1440,390,320]) {
      await page.setViewportSize({width,height:900});
      await page.locator('#editor-draft-control').scrollIntoViewIfNeeded();
      check(await page.locator('#editor-dialog').evaluate(el => el.scrollWidth <= el.clientWidth), `Editor bounds ${scheme}/${width}`);
      check(await page.locator('#editor-submit').isVisible(), `Editor action visible ${scheme}/${width}`);
      await page.screenshot({path:`${prefix}-editor-${scheme}-${width}.png`});
    }
    await page.keyboard.press('Escape');
    await page.locator('#project-settings-trigger').click();
    await page.waitForFunction(() => !document.querySelector('#project-drafts').disabled);
    if (!await page.locator('.planning-settings').evaluate(el => el.open)) await page.locator('.planning-settings summary').click();
    await page.locator('#project-drafts').scrollIntoViewIfNeeded();
    check((await page.locator('.planning-settings').innerText()).includes('Existing drafts'), 'Settings explains existing drafts are preserved');
    await page.screenshot({path:`${prefix}-settings-${scheme}-320.png`});
    await page.keyboard.press('Escape');
    await page.setViewportSize({width:1440,height:1000});
    await row.locator('.issue-title').click();
    await page.locator('.draft-notice').waitFor();
  }
  check(errors.length === 0, `No runtime errors: ${errors.join(', ')}`);
  const result = {passed:checks.length,checks};
  console.log(JSON.stringify(result));
  return result;
}
