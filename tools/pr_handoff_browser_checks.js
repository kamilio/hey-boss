// Run with playwright-cli run-code --filename against an isolated issue web fixture.
async page => {
  const checks = [], errors = [];
  const check = (ok, label) => { if (!ok) throw Error(label); checks.push(label); };
  const prefix = `output/playwright/issue50/${page.context().browser().browserType().name()}`;
  page.on('pageerror', error => errors.push(error.message));
  await page.waitForFunction(() => typeof model !== 'undefined' && model.project);
  await page.evaluate(async () => {
    await api({action:'configure_project',prs_enabled:false,prompt_overrides:{}}, model.project.id);
    await api({action:'unassign',number:1,force:false}, model.project.id);
  });
  await page.reload({waitUntil:'domcontentloaded'});
  await page.waitForFunction(() => typeof model !== 'undefined' && model.project);
  await page.getByRole('button', {name:'Project settings', exact:true}).waitFor();
  const open = async () => {
    await page.getByRole('button', {name:'Project settings', exact:true}).click();
    await page.waitForFunction(() => !document.querySelector('#project-prompt').disabled && document.querySelector('#project-instructions-preview').getAttribute('aria-busy') === 'false');
  };
  const ready = async prs => page.waitForFunction(prs => {
    const el = document.querySelector('#project-instructions-preview');
    return el.getAttribute('aria-busy') === 'false' && el.textContent.includes(prs ? 'attach every PR' : 'push to main');
  }, prs);
  await page.setViewportSize({width:1440, height:1000});
  await open();
  await page.locator('#project-prs').uncheck();
  await ready(false);
  check(!(await page.locator('#project-instructions-preview').innerText()).includes('assign-to-boss'), 'Main delivery has no PR handoff');
  await page.locator('#project-prs').check();
  await ready(true);
  check(await page.locator('#project-pr-handoff-help').count() === 0, 'No forced PR instructions in editor');
  await page.locator('#project-prompt-prs').fill('Publish the fix PR for {{number}}.');
  await page.waitForFunction(() => document.querySelector('#project-instructions-preview').textContent.includes('Publish the fix PR for 1.'));
  const text = await page.locator('#project-instructions-preview').innerText();
  check(text.endsWith('Publish the fix PR for 1.'), 'Custom delivery ends the preview');
  check(!text.includes('PR handoff:') && !text.includes('Record delivery and verification evidence'), 'No hidden prompt additions');
  check(await page.locator('#project-prompt-prs').getAttribute('aria-describedby') === 'project-source-prs', 'Source help is associated with PR editor');
  await page.getByRole('button', {name:'Save', exact:true}).click();
  await page.locator('#project-settings-dialog').waitFor({state:'hidden'});
  await open();
  check(await page.locator('#project-prompt-prs').inputValue() === 'Publish the fix PR for {{number}}.', 'Custom delivery instructions persist');
  check((await page.locator('#project-instructions-preview').innerText()).endsWith('Publish the fix PR for 1.'), 'Saved preview preserves custom delivery exactly');
  for (const scheme of ['light','dark']) {
    await page.emulateMedia({colorScheme:scheme});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width, height:width===1440?1000:844});
      check(await page.locator('#project-settings-dialog').evaluate(el => el.scrollWidth<=el.clientWidth && el.getBoundingClientRect().right<=innerWidth), `Settings fit ${scheme} ${width}`);
      await page.locator('#project-prompt-prs').evaluate(el => el.scrollIntoView({block:'center'}));
      check(await page.locator('#project-prompt-prs').evaluate(el => {
        const r=el.getBoundingClientRect(), form=el.closest('form');
        return r.top>=form.querySelector('.dialog-heading').getBoundingClientRect().bottom && r.bottom<=form.querySelector('.dialog-footer').getBoundingClientRect().top;
      }), `PR editor readable above footer ${scheme} ${width}`);
      await page.screenshot({path:`${prefix}-help-${scheme}-${width}.png`});
      await page.locator('#project-instructions-preview').scrollIntoViewIfNeeded();
      check(await page.locator('.settings-preview').evaluate(el => el.scrollWidth<=el.clientWidth), `Preview fits ${scheme} ${width}`);
      check(await page.getByRole('button', {name:'Save', exact:true}).isVisible(), `Settings footer accessible ${scheme} ${width}`);
      await page.screenshot({path:`${prefix}-preview-${scheme}-${width}.png`});
    }
  }
  await page.keyboard.press('Escape');
  check(await page.locator('#project-settings-trigger').evaluate(el => el===document.activeElement), 'Escape restores keyboard focus');
  await page.getByRole('link', {name:'PR handoff fixture', exact:true}).click();
  await page.getByRole('heading', {name:'PR handoff fixture #1', exact:true}).waitFor();
  const bossButton = page.getByRole('button', {name:'Assign to Boss', exact:true});
  if (await bossButton.count()) await bossButton.click();
  await page.waitForFunction(() => document.querySelector('.assignee-line')?.textContent.includes('Boss'));
  check(await page.locator('.state-pill.open').innerText() === 'Open', 'Boss handoff remains visibly open');
  check(await page.locator('.assignee-line > span[title]:not(.avatar)').innerText() === 'Boss', 'Boss owns the ready issue');
  check(await page.getByRole('combobox', {name:'Purpose of PR https://github.com/example/repo/pull/50',exact:true}).inputValue() === 'fix', 'Actual fix purpose survives handoff');
  check(await page.getByRole('combobox', {name:'Purpose of PR https://github.com/example/repo/pull/51',exact:true}).inputValue() === 'supporting-evidence', 'Supporting evidence purpose survives handoff');
  check((await page.locator('#comments').innerText()).includes('Existing verification history'), 'Previous comments survive handoff');
  for (const width of [1440,390,320]) {
    await page.setViewportSize({width,height:1000});
    check(await page.evaluate(() => document.documentElement.scrollWidth<=innerWidth), `Open Boss issue fits ${width}`);
    await page.screenshot({path:`${prefix}-open-boss-${width}.png`,fullPage:true});
  }
  check(errors.length===0, `No browser runtime errors: ${errors.join(', ')}`);
  return {passed:checks.length,checks};
}
