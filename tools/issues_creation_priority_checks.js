// Run with playwright-cli against an isolated issue web server.
async page => {
  page.setDefaultTimeout(10000);
  page.removeAllListeners('dialog');
  page.on('dialog', dialog => dialog.accept().catch(() => {}));
  const origin = await page.evaluate(() => location.origin), checks = [], errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  await page.waitForFunction(() => model.csrf && model.project);
  const project = await page.evaluate(async () => {
    const name = 'Creation priority QA ' + crypto.randomUUID();
    for (let n = 1; n <= 25; n++)
      await api({action:'create', title:'Existing priority ' + n, body:'', labels:['ready']}, name);
    return 'named:' + name;
  });
  const go = async (suffix = '') => {
    await page.goto(origin + '/#project=' + encodeURIComponent(project) + suffix);
    await page.waitForFunction(project => model.project?.id === project && model.signature, project);
  };
  const handle = number => page.locator(`#issue-list [data-move-issue="${number}"]`);
  const focused = async (number, name) => {
    await page.waitForFunction(number => document.activeElement?.dataset.moveIssue === String(number), number);
    check(await handle(number).evaluate(el => {
      const r = el.getBoundingClientRect();
      return r.top >= 0 && r.bottom <= innerHeight && r.left >= 0 && r.right <= innerWidth
        && el.contains(document.elementFromPoint(r.x + r.width/2, r.y + r.height/2));
    }), name + ': priority handle is focused and visible');
    check(await handle(number).evaluate(el => {
      const style = getComputedStyle(el);
      return style.outlineStyle === 'solid' && parseFloat(style.outlineWidth) >= 2 && style.opacity === '1';
    }), name + ': focus indicator is visible after pointer submission');
  };
  const create = async (quick, bottom) => {
    await page.locator(quick ? '#quick-issue-open' : '#new-issue').click();
    const prefix = quick ? '#quick-issue' : '#editor';
    await page.locator(prefix + (quick ? '-title' : '-subject')).fill('Keyboard-ready ' + Date.now());
    await page.locator(prefix + '-bottom').setChecked(bottom);
    await page.locator(prefix + '-submit').click();
    await page.waitForFunction(() => model.creation && document.activeElement?.dataset.moveIssue === String(model.creation.number));
    return await page.evaluate(() => model.creation.number);
  };
  const move = async (number, key) => {
    const before = await page.evaluate(number => model.issues.findIndex(i => i.number === number), number);
    await page.keyboard.press(key);
    await page.waitForFunction(({number, index}) => !model.orderSaving && model.issues[index]?.number === number,
      {number, index:before + (key === 'ArrowUp' ? -1 : 1)});
    await focused(number, key + ' immediately reorders');
  };
  for (const [label, width, height, scheme, quick, bottom] of [
    ['desktop light editor top',1440,1000,'light',false,false],
    ['desktop dark editor bottom',1440,1000,'dark',false,true],
    ['mobile light quick top',390,844,'light',true,false],
    ['mobile dark quick bottom',320,700,'dark',true,true],
  ]) {
    await page.setViewportSize({width, height});
    await page.emulateMedia({colorScheme:scheme, reducedMotion:'reduce'});
    await go();
    const number = await create(quick, bottom);
    await focused(number, label);
    check(await page.evaluate(({number,bottom}) => (bottom ? model.issues.at(-1) : model.issues[0]).number === number, {number,bottom}), label + ': correct initial priority');
    await page.screenshot({path:'output/playwright/issue95/' + label.replaceAll(' ','-') + '.png'});
    await move(number, bottom ? 'ArrowUp' : 'ArrowDown');
    await move(number, bottom ? 'ArrowDown' : 'ArrowUp');
    await page.evaluate(() => renderList({issues:model.issues, order_version:model.orderVersion}));
    await focused(number, label + ': list refresh');
    await page.waitForFunction(() => !model.creation);
    await focused(number, label + ': highlight expiry');
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), label + ': no horizontal overflow');
    check(await page.evaluate(async () => JSON.stringify((await api(listOperation())).issues.map(i => i.number)) === JSON.stringify(model.issues.map(i => i.number))), label + ': priority persisted');
  }
  await go('&label=ready&owner=mine&state=closed');
  const filtered = await create(false, false);
  await focused(filtered, 'Creation clears excluding filters');
  check(await page.evaluate(() => model.route.state === 'open' && model.route.owner === 'all' && !model.route.label), 'New issue is revealed in the open queue');
  await move(filtered, 'ArrowDown');
  await page.locator('#new-issue').click();
  await page.evaluate(() => window.cancelFocus = model.editor.returnFocus);
  await page.locator('#editor-cancel').click();
  check(await page.evaluate(() => window.cancelFocus === document.activeElement), 'Cancelling restores the original focus');
  await page.locator(`[data-issue-number="${filtered}"] .issue-title`).click();
  await page.locator('[data-edit]').click();
  await page.locator('#editor-subject').fill('Edited priority issue');
  await page.locator('#editor-submit').click();
  await page.waitForFunction(() => !model.editor && model.detail?.issue.title === 'Edited priority issue');
  check(await page.evaluate(number => model.route.issue === number, filtered), 'Editing retains issue detail');
  const fromDetail = await create(true, false);
  await focused(fromDetail, 'Quick Add from detail reveals the priority handle');
  await move(fromDetail, 'ArrowDown');
  const destination = await page.evaluate(async () => (await api({action:'create', title:'Destination issue', body:'', labels:[]}, 'Priority destination ' + crypto.randomUUID())).project);
  await page.locator('#quick-issue-open').click();
  await page.locator('#quick-issue-title').fill('Across projects @"' + destination.name + '"');
  await page.locator('#quick-issue-submit').click();
  await page.waitForFunction(project => model.project?.id === project && model.creation, destination.id);
  const across = await page.evaluate(() => model.creation.number);
  await focused(across, 'Cross-project Quick Add focuses the destination issue');
  await move(across, 'ArrowDown');
  check(errors.length === 0, 'No JavaScript runtime errors: ' + errors.join('; '));
  return {passed:checks.length, checks};
}
