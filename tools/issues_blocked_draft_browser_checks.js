// Run with playwright-cli against an isolated issue server on port 4799.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.setDefaultTimeout(30000);
  page.removeAllListeners('dialog');
  page.on('dialog', dialog => dialog.accept());
  await page.route('**/api/inbox', route => route.fulfill({json:{ok:true,tasks:[],unread:0}}));
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://127.0.0.1:4799/#project=named%3AIssue%20133%20QA');
  await page.waitForFunction(() => model.project?.id === 'named:Issue 133 QA');
  const action = operation => page.evaluate(operation => api(operation, model.project.id), operation);
  const issue = await action({action:'create',title:'Refine the onboarding plan',body:'## Next steps\n\nReview the first visit and keep the remaining work linked.',labels:['design']});
  const number = issue.issue.number;
  await action({action:'block',number,force:false});
  const open = async () => {
    await page.goto(`http://127.0.0.1:4799/#project=named%3AIssue%20133%20QA&view=issues&issue=${number}&state=blocked&owner=all`);
    await page.reload();
    await page.locator('.issue-readiness').waitFor();
  };
  await open();
  const move = page.getByRole('button',{name:'Move to draft',exact:true});
  check(await move.isEnabled(), 'Blocked detail offers Move to draft');
  await page.locator('#comment-body').fill('Keep this unsent review note.');
  const failed = async route => route.request().postDataJSON()?.operation?.draft === true
    ? route.fulfill({status:409,contentType:'application/json',body:JSON.stringify({ok:false,error:{code:'conflict',message:'Issue changed. Refresh and retry.'}})}) : route.continue();
  await page.route('**/api/action',failed);
  await move.click();
  await page.locator('#draft-error').filter({hasText:'Issue changed'}).waitFor();
  check(await move.isEnabled(), 'Conflict keeps action retryable');
  check(await page.locator('.state-pill.blocked').isVisible(), 'Conflict preserves blocked state');
  check(await page.locator('#comment-body').inputValue() === 'Keep this unsent review note.', 'Conflict preserves comment');
  await page.unroute('**/api/action',failed);
  await move.focus();
  await page.keyboard.press('Enter');
  await page.locator('.draft-notice').waitFor();
  check(await page.locator('.state-pill.draft').isVisible(), 'Keyboard action changes Blocked to Draft');
  check(await page.locator('#comment-body').inputValue() === 'Keep this unsent review note.', 'Draft transition preserves comment');
  check(await page.getByRole('button',{name:'Mark ready',exact:true}).evaluate(el => el === document.activeElement), 'Focus follows the readiness action');
  await page.reload();
  await page.locator('.draft-notice').waitFor();
  check(await page.locator('.state-pill.draft').isVisible(), 'Draft state persists after reload in blocked route');
  await page.getByRole('button',{name:'Mark ready',exact:true}).click();
  await move.waitFor();
  check(await page.locator('.state-pill.open').isVisible(), 'Manual block is cleared for later readiness');
  const child = await action({action:'create_subtask',number,title:'Review the empty state',body:'Check copy and spacing.',labels:[]});
  await open();
  check(await page.locator('.state-pill.blocked').isVisible(), 'Unfinished subtask blocks the parent');
  await action({action:'configure_project',drafts_enabled:false});
  await open();
  check(await move.isDisabled(), 'Disabled project drafts prevent transition');
  check((await page.locator('#readiness-help').innerText()).includes('disabled'), 'Disabled action explains why');
  await action({action:'configure_project',drafts_enabled:true});
  await open();
  for (const scheme of ['light','dark']) {
    await page.emulateMedia({colorScheme:scheme});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:900});
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `Blocked layout fits ${scheme}/${width}`);
      await move.scrollIntoViewIfNeeded();
      await page.screenshot({path:`output/playwright/issue133/blocked-${scheme}-${width}.png`,fullPage:true});
    }
  }
  await page.getByRole('button',{name:'Edit',exact:true}).click();
  await page.waitForFunction(() => !document.querySelector('#editor-draft').disabled);
  await page.locator('#editor-draft').check();
  check(await page.locator('#editor-submit').innerText() === 'Save draft', 'Blocked editor can save as draft');
  check(await page.locator('#editor-dialog').evaluate(el => el.scrollWidth <= el.clientWidth), 'Draft editor fits 320px');
  await page.screenshot({path:'output/playwright/issue133/editor-dark-320.png'});
  await page.locator('#editor-submit').click();
  await page.locator('.draft-notice').waitFor();
  check((await page.locator('.draft-notice').innerText()).includes('Unfinished blockers are kept'), 'Draft explains dependency preservation');
  for (const scheme of ['light','dark']) {
    await page.emulateMedia({colorScheme:scheme});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:900});
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `Draft layout fits ${scheme}/${width}`);
      await page.screenshot({path:`output/playwright/issue133/draft-${scheme}-${width}.png`,fullPage:true});
    }
  }
  await page.getByRole('button',{name:'Mark ready',exact:true}).click();
  await page.locator('.state-pill.blocked').waitFor();
  check((await page.locator('#toast').innerText()).includes('unfinished blockers'), 'Ready confirmation accurately describes blocked outcome');
  await move.click();
  await page.locator('.draft-notice').waitFor();
  const state = await action({action:'view',number});
  check(state.issue.blocked_by.length === 1, 'Subtask relationship survives the round trip');
  await action({action:'close',number:child.issue.number,force:false});
  await open();
  check(await page.locator('.state-pill.draft').isVisible(), 'Resolving dependency keeps the issue in Draft');
  await action({action:'block',number,force:false});
  await open();
  await page.getByRole('button',{name:'Edit',exact:true}).click();
  await page.waitForFunction(() => !document.querySelector('#editor-draft').disabled);
  check(await page.locator('#editor-draft').isChecked(), 'Previously drafted blocked issue keeps its editor choice');
  await page.locator('#editor-submit').click();
  await page.locator('.draft-notice').waitFor();
  check(await page.locator('.state-pill.draft').isVisible(), 'Saving an already checked draft clears its manual block');
  check(errors.length === 0, `No browser errors: ${errors.join(', ')}`);
  console.log(JSON.stringify({passed:checks.length,checks}));
  return {passed:checks.length,checks};
}
