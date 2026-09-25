// Run with playwright-cli against an isolated issue server (never production).
async page => {
  page.setDefaultTimeout(30000);
  const checks = [], errors = [], dialogs = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.removeAllListeners('dialog');
  page.on('dialog', dialog => { dialogs.push(dialog.type()); dialog.accept().catch(() => {}); });
  page.on('pageerror', error => errors.push(error.message));
  const base = await page.evaluate(() => location.origin);
  if (!['http://127.0.0.1:59661', 'http://127.0.0.1:52061'].includes(base)) throw Error('Synthetic fixture required');
  const paired = base.endsWith(':52061'), path = paired ? '/issues' : '/';
  const surface = paired ? 'paired' : 'native';
  await page.route('**/api/inbox', route => route.fulfill({json:{ok:true,tasks:[],unread_count:0}}));
  if (paired) {
    const {code} = await (await page.request.get(base + '/fixture-pairing')).json();
    await page.request.post(base + '/api/pair', {data:{code}});
  }
  const ready = () => page.waitForFunction(() => model.csrf && model.project && (model.route.issue ? model.detail : model.signature));
  await page.goto(base + path); await ready();
  const project = await page.evaluate(async () => (await api({action:'create',title:'Exit navigation fixture',body:'Saved description',labels:[]},'Exit navigation '+crypto.randomUUID())).project.id);
  const detail = async () => {
    await page.goto(base + path + '#project=' + encodeURIComponent(project) + '&issue=1');
    await page.locator('#comment-body').waitFor();
  };
  const noWarning = async name => {
    check(!await page.evaluate(() => { const e = new Event('beforeunload',{cancelable:true}); dispatchEvent(e); return e.defaultPrevented; }), name);
  };
  const reload = async () => { await page.reload(); await ready(); };
  await detail();
  await page.locator('#comment-body').fill('Comment draft survives leaving the issue');
  await noWarning('Visible comment never blocks leaving');
  await reload();
  check(await page.locator('#comment-body').inputValue() === 'Comment draft survives leaving the issue', 'Reload restores desktop comment draft');
  await page.locator('[data-back]').click();
  await noWarning('Hidden comment never blocks list navigation');
  await reload(); await detail();
  check(await page.locator('#comment-body').inputValue() === 'Comment draft survives leaving the issue', 'List navigation retains comment draft');
  await page.getByRole('button',{name:'Edit',exact:true}).click();
  await page.locator('#editor-subject').fill('Edited title survives reload');
  await page.locator('#editor-body').fill('Edited body survives reload');
  await noWarning('Existing issue editor never blocks leaving');
  await reload();
  await page.getByRole('button',{name:'Edit',exact:true}).click();
  check(await page.locator('#editor-subject').inputValue() === 'Edited title survives reload', 'Reload restores edited title');
  check(await page.locator('#editor-body').inputValue() === 'Edited body survives reload', 'Reload restores edited body');
  await page.keyboard.press('Escape');
  await page.locator('[data-back]').click();
  await page.locator('#new-issue').click();
  await noWarning('Empty new issue editor never blocks leaving');
  await page.locator('#editor-subject').fill('New issue draft survives reload');
  await page.locator('#editor-body').fill('New description survives reload');
  await noWarning('New issue draft never blocks leaving');
  await reload(); await page.locator('#new-issue').click();
  check(await page.locator('#editor-subject').inputValue() === 'New issue draft survives reload', 'Reload restores new title');
  check(await page.locator('#editor-body').inputValue() === 'New description survives reload', 'Reload restores new body');
  await page.keyboard.press('Escape'); await page.locator('#new-issue').click();
  check(await page.locator('#editor-subject').inputValue() === 'New issue draft survives reload', 'Escape retains draft without confirmation');
  for (const scheme of ['light','dark']) {
    await page.emulateMedia({colorScheme:scheme,reducedMotion:'reduce'});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:900});
      check(await page.locator('#editor-dialog').evaluate(el => el.scrollWidth <= el.clientWidth), `Editor fits ${scheme}/${width}`);
      check(await page.locator('#editor-submit').isVisible(), `Save action visible ${scheme}/${width}`);
      await page.screenshot({path:`output/playwright/issue362/${surface}-${scheme}-${width}.png`});
    }
  }
  await page.locator('#editor-submit').click();
  await page.waitForFunction(() => !document.querySelector('#editor-dialog').open);
  await reload(); await page.locator('#new-issue').click();
  check(await page.locator('#editor-subject').inputValue() === '', 'Successful submission clears saved draft');
  await page.keyboard.press('Escape');
  // Disabled/quota-exhausted storage must never prevent typing or navigation.
  await page.evaluate(() => { Storage.prototype.setItem = () => { throw new DOMException('Synthetic storage failure','QuotaExceededError'); }; });
  await page.locator('#new-issue').click();
  await page.locator('#editor-subject').fill('Storage unavailable');
  await noWarning('Storage failure never blocks leaving');
  await reload();
  check(dialogs.length === 0, 'No browser dialogs across reloads, navigation, and dismissal: ' + dialogs.join(','));
  check(errors.length === 0, 'No browser errors: ' + errors.join('; '));
  return {surface,passed:checks.length,checks};
}
