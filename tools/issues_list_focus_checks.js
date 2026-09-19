// Background issue changes must not interrupt keyboard navigation.
async page => {
  page.removeAllListeners('dialog');
  page.on('dialog', dialog => dialog.accept().catch(() => {}));
  const origin = await page.evaluate(() => location.origin), checks = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name) };
  await page.goto(origin + '/?focus-qa=' + Date.now());
  await page.waitForFunction(() => model.csrf && model.project);
  const project = await page.evaluate(async () => {
    const name = 'List focus QA ' + Date.now();
    for (let number = 1; number <= 35; number++)
      await api({ action: 'create', title: 'Focus issue ' + number, body: '', labels: ['ready'] }, name);
    return 'named:' + name;
  });
  await page.goto(origin + '/?focus-qa=' + Date.now() + '#project=' + encodeURIComponent(project));
  await page.waitForFunction(project => model.project?.id === project && model.signature && model.issues.length === 35, project);
  const handle = page.locator('[data-move-issue="30"]');
  await page.evaluate(() => {window.scrollProbe=[];window.addEventListener('scroll',()=>scrollProbe.push({time:performance.now(),y:scrollY,focusTop:document.activeElement?.getBoundingClientRect().top}))});
  await handle.scrollIntoViewIfNeeded();
  await handle.focus();
  await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
  const position = await page.evaluate(() => scrollY);
  const beforeTop = await handle.evaluate(el => el.getBoundingClientRect().top);
  await page.evaluate(async () => {
    await api({ action: 'edit', number: 1, title: 'A much longer headline from another session '.repeat(10), body: null, add_labels: [], remove_labels: [], if_version: null });
    await refresh(false);
  });
  check(await handle.evaluate(el => el === document.activeElement), 'Background edit preserves reorder focus');
  const after = await page.evaluate(() => scrollY);
  const afterTop = await handle.evaluate(el=>el.getBoundingClientRect().top);
  if(Math.abs(afterTop-beforeTop)>=2) throw Error('Focused row moved: '+JSON.stringify({before:position,after,beforeTop,afterTop,trace:await page.evaluate(()=>scrollProbe)}));
  check(true,'Taller background update preserves focused row position');
  await page.keyboard.press('ArrowUp');
  await page.waitForFunction(() => !model.orderSaving && model.issues[28]?.number === 30);
  check(true, 'Keyboard can continue reordering after background update');
  const title = page.locator('[data-issue-number="30"] .issue-title');
  await title.focus();
  await page.evaluate(async () => {
    await api({ action: 'edit', number: 30, title: 'Changed focused issue', body: null, add_labels: [], remove_labels: [], if_version: null });
    await refresh(false);
  });
  check(await title.evaluate(el => el === document.activeElement), 'Changed issue title preserves navigation focus');
  const label = page.locator('[data-issue-number="30"] .list-label-filter');
  await label.focus();
  await page.evaluate(async () => { await api({ action: 'comment', number: 1, body: 'External update' }); await refresh(false) });
  check(await label.evaluate(el => el === document.activeElement), 'Background comment preserves label-link focus');
  await page.evaluate(async () => { await api({ action: 'delete', number: 30, force: false }); await refresh(false) });
  check(await page.locator('#issue-search').evaluate(el => el === document.activeElement), 'Removed focused issue returns focus to search');
  check(await page.locator('.issue-row').count() === 34, 'Refresh removes deleted issue without changing other tickets');
  return { passed: checks.length, checks };
}
