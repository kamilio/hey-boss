// Run with playwright-cli run-code --filename against an isolated issue server on 4782.
async page => {
  page.setDefaultTimeout(10000);
  const origin = await page.evaluate(() => location.origin);
  const checks = [], errors = [], writes = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => {
    if (request.url().endsWith('/api/action') && request.postDataJSON()?.operation?.action === 'set_yolo') writes.push(request.postDataJSON());
  });
  await page.route("**/api/inbox", route => route.fulfill({json:{ok:true,tasks:[],unread:0}}));
  const seed = await page.evaluate(async () => {
    const value = await api({action:'create',title:'Deploy the next release',body:'Check the release on desktop and phone before installing.',labels:['ready']}, `YOLO QA ${Date.now()}`);
    return {project:value.project.id,number:value.issue.number};
  });
  const detail = `${origin}/#project=${encodeURIComponent(seed.project)}&issue=${seed.number}`;
  await page.goto(detail);
  const openTags = async () => {
    if (!await page.locator('#issue-tag-picker').count()) await page.getByRole('button', {name:'Assign tags',exact:true}).click();
    await page.locator('[data-issue-tag="yolo"]').waitFor();
  };
  const yolo = page.locator('[data-issue-tag="yolo"]');
  const badge = page.locator('.side-labels .yolo-badge');
  await openTags();
  check(await page.locator('.agent-permissions, [data-yolo-toggle]').count() === 0, 'No separate permissions concept');
  check(!await yolo.isChecked(), 'YOLO is available before any issue uses it');
  check((await yolo.locator('..').locator('.yolo-badge').getAttribute('title')).includes('No sandbox or approval prompts'), 'Tag effect is available in its tooltip');
  check(!(await yolo.locator('..').innerText()).includes('No sandbox'), 'YOLO has no inline warning text');
  await page.locator('#issue-tag-search').fill('yolo');
  check(await page.locator('[data-new-issue-tag="yolo"]').count() === 0, 'Searching finds the existing special tag');
  await yolo.focus(); await page.keyboard.press('Space');
  await badge.waitFor();
  await page.waitForFunction(() => !document.querySelector('[data-issue-tag="yolo"]')?.disabled);
  check(await yolo.isChecked(), 'Keyboard selection adds YOLO through the ordinary tag picker');
  check(!await page.locator('#confirm-dialog').evaluate(el => el.open), 'YOLO toggles without a confirmation dialog');
  check(writes[0].operation.if_version === 1 && !!writes[0].request_id, 'Special tag writes are guarded and idempotent');
  check(await badge.locator('svg').count() === 1, 'YOLO has a lightning icon');
  check(await page.locator('[data-remove-issue-tag="yolo"]').count() === 1, 'YOLO has the ordinary removal control');
  await page.locator('#issue-tag-search').fill('release');
  await page.locator('[data-new-issue-tag="release"]').click();
  await page.locator('[data-remove-issue-tag="release"]').waitFor();
  check(await badge.count() === 1, 'Creating an ordinary tag preserves YOLO');
  await page.getByRole('button', {name:'Close tag picker'}).click();
  await page.getByRole('button', {name:'Edit',exact:true}).click();
  await page.locator('#editor-subject').fill('Deploy the reviewed release');
  await page.locator('#editor-submit').click();
  await page.waitForFunction(() => !document.querySelector('#editor-dialog').open);
  await badge.waitFor();
  check(await badge.count() === 1, 'Title editing preserves YOLO');
  await page.reload(); await badge.waitFor();
  check(await badge.count() === 1, 'Special tag survives reload');
  for (const [name,width,height,scheme] of [
    ['desktop-light',1440,1000,'light'], ['desktop-dark',1440,1000,'dark'],
    ['mobile-light',390,844,'light'], ['mobile-dark',390,844,'dark'], ['narrow-mobile',320,760,'light'],
  ]) {
    await page.setViewportSize({width,height}); await page.emulateMedia({colorScheme:scheme});
    await page.getByRole('button',{name:'Assign tags',exact:true}).scrollIntoViewIfNeeded();
    await openTags();
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth && [...document.querySelectorAll('#issue-tag-picker,.side-labels')].every(el => {const r=el.getBoundingClientRect(); return r.width>0 && r.left>=0 && r.right<=innerWidth && (el.id!=="issue-tag-picker" || (r.top>=-1 && r.bottom<=innerHeight+1));})), `${name}: tags and picker fit without overflow`);
    await page.screenshot({path:`output/playwright/issue119/${name}.png`});
    await page.getByRole('button', {name:'Close tag picker'}).click();
  }
  await openTags();
  await page.route('**/api/action', async route => {
    if (route.request().postDataJSON()?.operation?.action === 'set_yolo') await route.fulfill({status:503,json:{ok:false,error:{code:'unavailable',message:'Test server unavailable. Retry.'}}});
    else await route.continue();
  });
  await yolo.click();
  await page.locator('#issue-tag-error').waitFor({state:'visible'});
  check(await yolo.isChecked() && await yolo.isEnabled(), 'Failed removal restores saved checkbox and allows retry');
  await page.unroute('**/api/action');
  await page.evaluate(async ({project,number}) => {await api({action:'edit',number,title:null,body:'Concurrent description update',add_labels:[],remove_labels:[]},project);}, seed);
  await yolo.click();
  await page.waitForFunction(() => document.querySelector('#issue-tag-error')?.textContent.includes('Refresh and retry'));
  check(await yolo.isChecked(), 'Stale revision cannot overwrite permissions');
  await yolo.focus(); await page.keyboard.press('Space');
  await page.waitForFunction(() => !model.detail.issue.labels.includes('yolo'));
  check(await page.locator('#issue-tag-error').isHidden(), 'Keyboard retry removes YOLO and clears the error');
  await yolo.click(); await badge.waitFor();
  await page.getByRole('button', {name:'Close tag picker'}).click();
  await page.getByRole('button',{name:'Remove yolo tag',exact:true}).click();
  await page.waitForFunction(() => !model.detail.issue.labels.includes('yolo'));
  check(await badge.count() === 0, 'Chip × removes YOLO without another permissions panel');
  await openTags(); await yolo.click(); await badge.waitFor();
  await page.goto(`${origin}/#project=${encodeURIComponent(seed.project)}`);
  const listBadge = page.locator('.issue-row .yolo-badge'); await listBadge.waitFor();
  for (const [width,scheme] of [[1440,'light'],[390,'dark']]) {
    await page.setViewportSize({width,height:1000}); await page.emulateMedia({colorScheme:scheme});
    check(await page.evaluate(() => document.documentElement.scrollWidth<=innerWidth), `List ${width}px fits`);
    await page.screenshot({path:`output/playwright/issue119/list-${width}.png`,fullPage:true});
  }
  await listBadge.click(); await page.waitForFunction(() => model.route.label === 'yolo');
  check(await page.locator('.issue-row').count() === 1, 'Special badge works as a label filter');
  await page.goto(detail); await badge.waitFor();
  await page.evaluate(() => {model.actor.id='codex:visual-test'; document.querySelector('.tag-section').outerHTML=renderTagSidebar(model.detail.issue);});
  await openTags();
  check(await yolo.isDisabled(), 'Agents can see the special tag without permission controls');
  check(await page.locator('[data-remove-issue-tag="yolo"]').count() === 0, 'Agents cannot remove the special tag');
  check(errors.length === 0, 'No browser exceptions');
  return {passed:checks.length,checks,seed};
}
