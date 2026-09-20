// Replace AXE_SOURCE with a JSON string containing axe.min.js; run via playwright-cli.
async page => {
  page.setDefaultTimeout(90000);
  page.setDefaultNavigationTimeout(90000);
  page.removeAllListeners('dialog');
  page.on('dialog', dialog => dialog.accept());
  await page.goto('about:blank');
  await page.goto('http://127.0.0.1:4794/#project=named%3ADraft%20QA',{waitUntil:'domcontentloaded'});
  await page.waitForFunction(() => model.project?.id === 'named:Draft QA' && model.issues?.length);
  await page.evaluate(() => localStorage.clear());
  await page.evaluate(/* AXE_SOURCE */);
  const reports = [];
  const audit = async name => {
    const report = await page.evaluate(async () => {
      const result = await axe.run(document,{runOnly:{type:'tag',values:['wcag2a','wcag2aa','wcag21aa','best-practice']}});
      return {violations:result.violations.map(v => ({id:v.id,nodes:v.nodes.map(n => ({target:n.target,summary:n.failureSummary}))})),passes:result.passes.length};
    });
    reports.push({name,...report});
  };
  const number = await page.evaluate(() => model.issues.find(i => i.draft)?.number);
  if (!number) throw Error('Seed the synthetic draft fixture before this audit');
  for (const scheme of ['light','dark']) for (const width of [1440,320]) {
    await page.emulateMedia({colorScheme:scheme});
    await page.setViewportSize({width,height:844});
    await audit(`list-${scheme}-${width}`);
    await page.locator(`.issue-row[data-issue-number="${number}"] .issue-title`).click();
    await page.locator('.draft-notice').waitFor();
    await audit(`detail-${scheme}-${width}`);
    await page.getByRole('button',{name:'Edit',exact:true}).click();
    await page.waitForFunction(() => !document.querySelector('#editor-draft').disabled);
    await page.locator('#editor-draft-control').scrollIntoViewIfNeeded();
    await audit(`editor-${scheme}-${width}`);
    await page.keyboard.press('Escape');
    await page.locator('#project-settings-trigger').click();
    await page.waitForFunction(() => !document.querySelector('#project-drafts').disabled);
    if (!await page.locator('.planning-settings').evaluate(el => el.open)) await page.locator('.planning-settings summary').click();
    await page.locator('#project-drafts').scrollIntoViewIfNeeded();
    await audit(`settings-${scheme}-${width}`);
    await page.keyboard.press('Escape');
    await page.getByRole('button',{name:'All issues',exact:true}).click();
    await page.locator(`.issue-row[data-issue-number="${number}"]`).waitFor();
  }
  await page.evaluate(reports => { window.__draftAxeReports = reports; },reports);
  const failures = reports.filter(r => r.violations.length);
  if (failures.length) throw Error(JSON.stringify(failures));
  return reports;
}
