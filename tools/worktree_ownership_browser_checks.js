async (page, {base, shots}) => {
  const checks = [], errors = [];
  const check = (ok, label) => { if (!ok) throw Error(label); checks.push(label); };
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(base, {waitUntil:'domcontentloaded'});
  await page.waitForFunction(() => model.project?.id);
  await page.getByRole('button', {name:'Project settings', exact:true}).click();
  await page.waitForFunction(() => !document.querySelector('#project-prompt').disabled);
  await page.locator('#project-worktree').check();
  await page.locator('#project-preview-workspace').selectOption('worktree');
  const ready = () => page.waitForFunction(() => {
    const el = document.querySelector('#project-instructions-preview');
    return el.getAttribute('aria-busy') === 'false' && el.textContent.includes('owner-authorized cleanup');
  });
  await ready();
  const preview = page.locator('#project-instructions-preview');
  const initial = await preview.innerText();
  check(initial.includes('git worktree add --lock --reason'), 'New worktree starts locked');
  check(initial.includes('queued') && initial.includes('session'), 'Queued ownership is visible in instructions');
  check(initial.includes('staged') && initial.includes('duplicate validators'), 'Resume preserves work and validators');
  check(initial.includes('receipts'), 'Cleanup preserves receipts');
  check(!initial.includes('{{worktree_'), 'Worktree variables expand');
  for (const theme of ['light','dark']) {
    await page.emulateMedia({colorScheme:theme});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:1000});
      await preview.scrollIntoViewIfNeeded();
      check(await page.locator('#project-settings-dialog').evaluate(el => {
        const r=el.getBoundingClientRect(); return r.left>=0 && r.right<=innerWidth && r.bottom<=innerHeight;
      }), `Dialog fits ${theme}/${width}`);
      check(await preview.evaluate(el => el.scrollWidth<=el.clientWidth), `Ownership text wraps ${theme}/${width}`);
      check(await page.getByRole('button',{name:'Save',exact:true}).isVisible(), `Save remains reachable ${theme}/${width}`);
      await page.screenshot({path:`${shots}/ownership-${theme}-${width}.png`});
    }
  }
  await page.locator('#project-preview-workspace').selectOption('checkout');
  await page.waitForFunction(() => {
    const el=document.querySelector('#project-instructions-preview');
    return el.getAttribute('aria-busy')==='false' && el.textContent.includes("project's existing checkout");
  });
  check(!(await preview.innerText()).includes('git worktree add --lock'), 'Existing-checkout branch stays scoped');
  await page.locator('#project-preview-workspace').selectOption('worktree');
  await ready();
  check(await preview.innerText()===initial, 'Switching branches preserves deterministic instructions');
  await page.keyboard.press('Escape');
  check(await page.locator('#project-settings-trigger').evaluate(el => el===document.activeElement), 'Escape restores keyboard focus');
  check(errors.length===0, 'No browser runtime errors: '+errors.join('; '));
  if (checks.length!==33) throw Error(`Incomplete browser task graph ${checks.length}/33`);
  return {completed:checks.length,expected:33,checks};
}
