// Run with playwright-cli run-code --filename against an isolated server on 4782.
async page => {
  page.setDefaultTimeout(10000);
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  const seed = await page.evaluate(async () => {
    const value = await api({action:'create',title:'Retry a task with explicit permissions',body:'Boss controls permissions for the next attempt.',labels:['ready']}, 'YOLO QA');
    return {project:value.project.id,number:value.issue.number};
  });
  const detail = `http://127.0.0.1:4782/#project=${encodeURIComponent(seed.project)}&issue=${seed.number}`;
  await page.goto(detail);
  const toggle = page.locator('[data-yolo-toggle]');
  await toggle.waitFor();
  check(await toggle.textContent() === 'Enable YOLO…', 'Auto is the default');
  await toggle.click();
  await page.locator('#confirm-dialog[open]').waitFor();
  check((await page.locator('#confirm-message').textContent()).includes('full access'), 'Confirmation explains full access');
  check(await page.locator('#confirm-cancel').evaluate(el => el === document.activeElement), 'Cancel receives initial keyboard focus');
  await page.keyboard.press('Escape');
  check(await toggle.getAttribute('aria-pressed') === 'false', 'Cancelling leaves Auto enabled');
  await toggle.click();
  await page.locator('#confirm-submit').click();
  await page.waitForFunction(() => document.querySelector('[data-yolo-toggle]')?.getAttribute('aria-pressed') === 'true');
  check(await page.locator('.side-labels .yolo-badge').count() === 1, 'Special YOLO badge appears');
  check(await page.locator('[data-remove-issue-tag="yolo"]').count() === 0, 'Regular tags cannot remove YOLO');
  await page.getByRole('button', {name:'Edit',exact:true}).click();
  await page.locator('#editor-subject').fill('Explicit permissions survive ordinary editing');
  await page.locator('#editor-submit').click();
  await page.waitForFunction(() => !document.querySelector('#editor-dialog').open);
  check(await toggle.getAttribute('aria-pressed') === 'true', 'Editing issue title preserves YOLO');
  await page.reload();
  await toggle.waitFor();
  check(await toggle.getAttribute('aria-pressed') === 'true', 'Boss choice survives reload');
  await page.getByRole('button', {name:'Assign tags',exact:true}).click();
  check(await page.locator('[data-issue-tag="yolo"]').count() === 0, 'YOLO is excluded from ordinary tag picker');
  await page.getByRole('button', {name:'Close tag picker'}).click();
  for (const [name,width,height,scheme] of [
    ['desktop-light',1440,1000,'light'], ['desktop-dark',1440,1000,'dark'],
    ['mobile-light',390,844,'light'], ['mobile-dark',390,844,'dark'], ['narrow-mobile',320,760,'light'],
  ]) {
    await page.setViewportSize({width,height});
    await page.emulateMedia({colorScheme:scheme});
    await toggle.scrollIntoViewIfNeeded();
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth && [...document.querySelectorAll('.agent-permissions, [data-yolo-toggle]')].every(el => {const r=el.getBoundingClientRect(); return r.width>0 && r.left>=0 && r.right<=innerWidth;})), `${name}: permissions fit without overflow`);
    await page.screenshot({path:`output/playwright/issue80/${name}.png`,fullPage:true});
  }
  // A failed write must leave the existing permission visible and allow retry.
  await page.route('**/api/action', async route => {
    if (route.request().postDataJSON()?.operation?.action === 'set_yolo') {
      await route.fulfill({status:503,contentType:'application/json',body:JSON.stringify({ok:false,error:{code:'unavailable',message:'Test server unavailable. Retry.'}})});
    } else await route.continue();
  });
  await toggle.click();
  await page.locator('#yolo-error').waitFor({state:'visible'});
  check(await toggle.getAttribute('aria-pressed') === 'true' && await toggle.isEnabled(), 'Failed disable keeps saved choice and allows retry');
  await page.unroute('**/api/action');
  // Another writer advances the revision. The UI refreshes before a second try.
  await page.evaluate(async ({project,number}) => { await api({action:'edit',number,title:null,body:'Concurrent description update',add_labels:[],remove_labels:[]},project); },seed);
  await toggle.click();
  await page.waitForFunction(() => document.querySelector('#yolo-error')?.textContent.includes('Refresh and retry'));
  check(await toggle.getAttribute('aria-pressed') === 'true', 'Stale revision cannot overwrite Boss permissions');
  await toggle.focus();
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => document.querySelector('[data-yolo-toggle]')?.getAttribute('aria-pressed') === 'false');
  check(await page.locator('#yolo-error').isHidden(), 'Keyboard retry disables YOLO and clears stale error');
  await toggle.click();
  await page.locator('#confirm-submit').click();
  await page.waitForFunction(() => document.querySelector('[data-yolo-toggle]')?.getAttribute('aria-pressed') === 'true');
  await page.goto(`http://127.0.0.1:4782/#project=${encodeURIComponent(seed.project)}`);
  const badge = page.locator('.issue-row .yolo-badge');
  await badge.waitFor();
  for (const [width,scheme] of [[1440,'light'],[390,'dark']]) {
    await page.setViewportSize({width,height:1000}); await page.emulateMedia({colorScheme:scheme});
    check(await page.evaluate(() => document.documentElement.scrollWidth<=innerWidth), `List ${width}px fits`);
    await page.screenshot({path:`output/playwright/issue80/list-${width}.png`,fullPage:true});
  }
  await badge.click();
  await page.waitForFunction(() => model.route.label === 'yolo');
  check(await page.locator('.issue-row').count() === 1, 'Special badge still works as a label filter');
  await page.goto(detail);
  await toggle.waitFor();
  await page.evaluate(() => {model.actor.id='codex:visual-test'; document.querySelector('.agent-permissions').outerHTML=renderAgentPermissions(model.detail.issue);});
  check(await toggle.count() === 0, 'Agents see the mode without Boss controls');
  check(errors.length === 0, 'No browser exceptions');
  return {passed:checks.length,checks,seed};
}
