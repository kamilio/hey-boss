async page => {
  const checks = [], errors = [];
  const check = (ok, label) => { if (!ok) throw Error(label); checks.push(label); };
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://127.0.0.1:4782/', {waitUntil:'domcontentloaded'});
  await page.waitForFunction(() => model.project?.id === 'named:Workflow QA');
  await page.evaluate(async () => api({action:'configure_project',prompt:'Claim {{number}}.\n{{worktree_name}}\n{{worktree_path}}',worktree_enabled:true,prompt_overrides:{}}, model.project.id));
  await page.setViewportSize({width:1440,height:1000});
  await page.getByRole('button', {name:'Project settings',exact:true}).click();
  await page.waitForFunction(() => !document.querySelector('#project-prompt').disabled);
  await page.locator('#project-preview-workspace').selectOption('worktree');
  const ready = () => page.waitForFunction(() => {
    const el = document.querySelector('#project-instructions-preview');
    return el.getAttribute('aria-busy') === 'false' && el.textContent.includes('Reuse that worktree');
  });
  await ready();
  const text = await page.locator('#project-instructions-preview').textContent();
  check(text.includes('workflow-qa-fix-worktree-na-1'), 'Resolved bounded name includes project and issue number');
  const path = text.split('\n')[2];
  check(path.startsWith('/') && path.endsWith('/workflow-qa-fix-worktree-na-1'), 'Absolute path uses the matching directory name');
  check(text.includes('branch `workflow-qa-fix-worktree-na-1`'), 'Branch matches directory name');
  check(!text.includes('{{worktree_'), 'Variables expand in shared and default prompts');
  check((await page.locator('.settings-editor').innerText()).includes('{{worktree_path}}'), 'Variables explained in Workspace');
  await page.locator('#project-prompt-worktree').fill('Reuse {{worktree_name}} at {{worktree_path}}.');
  await page.waitForFunction(() => {
    const el = document.querySelector('#project-instructions-preview');
    return el.getAttribute('aria-busy') === 'false' && el.textContent.includes('Reuse workflow-qa-');
  });
  check(!(await page.locator('#project-instructions-preview').textContent()).includes('{{worktree_'), 'Custom branch expands variables');
  await page.locator('[data-reset-prompt=worktree]').click();
  await ready();
  check((await page.locator('#project-instructions-preview').textContent()) === text, 'Preview regenerates identical name and path');
  for (const theme of ['light','dark']) {
    await page.emulateMedia({colorScheme:theme});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:1000});
      check(await page.locator('#project-settings-dialog').evaluate(el => el.scrollWidth <= el.clientWidth && el.getBoundingClientRect().right <= innerWidth), `Dialog fits ${theme}/${width}`);
      check(await page.locator('.project-settings-body').evaluate(el => el.scrollWidth <= el.clientWidth), `Content fits ${theme}/${width}`);
      await page.locator('#project-worktree-help').scrollIntoViewIfNeeded();
      await page.screenshot({path:`output/playwright/issue79/workspace-${theme}-${width}.png`});
      await page.locator('#project-instructions-preview').scrollIntoViewIfNeeded();
      await page.screenshot({path:`output/playwright/issue79/preview-${theme}-${width}.png`});
      check(await page.getByRole('button',{name:'Save',exact:true}).isVisible(), `Footer accessible ${theme}/${width}`);
    }
  }
  await page.keyboard.press('Escape');
  check(await page.locator('#project-settings-trigger').evaluate(el => el === document.activeElement), 'Escape restores focus');
  check(errors.length === 0, `No page errors: ${errors.join(', ')}`);
  return {passed:checks.length,checks};
}
