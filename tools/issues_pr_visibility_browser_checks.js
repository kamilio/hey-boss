// Run with playwright-cli run-code --filename against an isolated issue server.
async page => {
  const checks = [], errors = [];
  const check = (ok, label) => { if (!ok) throw Error(label); checks.push(label); };
  page.setDefaultTimeout(15000);
  page.on('pageerror', error => errors.push(error.message));
  await page.waitForFunction(() => model.csrf && model.project);
  const {project, number} = await page.evaluate(async () => {
    const created = await api({action:'create', title:'Deliver changes directly to main', body:'Keep linked resources aligned with the project workflow.', labels:[]}, model.project.id);
    await api({action:'configure_project', prs_enabled:false}, created.project.id);
    return {project:created.project.id, number:created.issue.number};
  });
  const base = await page.evaluate(() => location.origin + location.pathname);
  const detail = `${base}#project=${encodeURIComponent(project)}&issue=${number}`;
  await page.goto(detail);
  await page.waitForFunction(number => model.detail?.issue.number === number && model.detail.prs_enabled === false, number);
  const absent = async () => {
    check(await page.getByRole('heading', {name:'Pull requests',exact:true}).count() === 0, 'Disabled workflow has no PR heading');
    check(await page.locator('#pr-form, .pr-links, [data-pr-purpose], [data-remove-pr]').count() === 0, 'Disabled workflow has no PR controls or empty panel');
    check(await page.locator('.commit-section').isVisible(), 'Commit section remains available');
    check(await page.locator('#issue-artifacts').isVisible() && await page.locator('#related-notices').isVisible(), 'Other linked resources remain visible');
  };
  await absent();
  for (const colorScheme of ['light','dark']) {
    await page.emulateMedia({colorScheme,reducedMotion:'reduce'});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:900});
      await page.locator('.issue-resources').scrollIntoViewIfNeeded();
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `No horizontal overflow at ${width}px/${colorScheme}`);
      check(await page.locator('.issue-resources').evaluate(el => {
        const sections = [...el.children].filter(child => child.getBoundingClientRect().height > 0);
        return sections.every((child, index) => !index || child.getBoundingClientRect().top >= sections[index-1].getBoundingClientRect().bottom - 1);
      }), `Linked resources do not overlap at ${width}px/${colorScheme}`);
    }
  }
  await page.setViewportSize({width:1440,height:1000});
  const settings = async enabled => {
    await page.getByRole('button', {name:'Project settings',exact:true}).click();
    await page.locator('#project-prs').setChecked(enabled);
    await page.getByRole('button', {name:'Save',exact:true}).click();
    await page.locator('#project-settings-dialog').waitFor({state:'hidden'});
    await page.waitForFunction(enabled => model.detail?.prs_enabled === enabled, enabled);
  };
  await settings(true);
  check(await page.locator('.pr-links').innerText() === 'No pull requests attached.', 'Enabling PRs restores the empty state');
  await page.getByText('Attach a pull request', {exact:true}).click();
  await page.locator('#pr-url').fill('https://github.com/example/repo/pull/123');
  await page.locator('#pr-purpose').selectOption('fix');
  await page.getByRole('button', {name:'Attach PR',exact:true}).click();
  await page.locator('[data-pr-purpose]').waitFor();
  check(await page.locator('[data-pr-purpose]').inputValue() === 'fix', 'Enabled workflow still attaches and classifies a PR');
  await settings(false);
  await absent();
  await page.reload();
  await page.waitForFunction(() => model.detail?.prs_enabled === false);
  await absent();
  check(await page.evaluate(() => model.detail.issue.pull_requests.length === 1), 'Disabling PRs preserves existing attachment data');
  await page.locator('#comment-body').fill('Unsent comment survives workflow updates.');
  await page.evaluate(async () => {
    await api({action:'configure_project',prs_enabled:true});
    while (model.polling) await new Promise(resolve => setTimeout(resolve,20));
    await refresh(true);
  });
  await page.locator('#update-banner').waitFor();
  check(await page.locator('#comment-body').inputValue() === 'Unsent comment survives workflow updates.', 'Background workflow changes preserve the comment draft');
  await page.getByRole('button', {name:'Load changes',exact:true}).click();
  await page.locator('[data-pr-purpose]').waitFor();
  check(await page.locator('#comment-body').inputValue() === 'Unsent comment survives workflow updates.', 'Loading the workflow change restores the comment draft');
  await page.locator('#comment-body').fill('');
  await page.evaluate(async number => {await api({action:'delete',number,force:false});await renderRoute();}, number);
  check(await page.locator('.pr-links a').count() === 1 && await page.locator('#pr-form, [data-pr-purpose], [data-remove-pr]').count() === 0, 'Deleted issue keeps enabled PR history read-only');
  await page.evaluate(async () => {await api({action:'configure_project',prs_enabled:false});await refresh(false);});
  await absent();
  check(errors.length === 0, `No browser exceptions: ${errors.join(', ')}`);
  return {passed:checks.length,checks,project};
}
