// Run through playwright-cli against a private companion viewer with a real fleet connection.
async page => {
  page.setDefaultTimeout(20000);
  const checks = [];
  const check = (condition, name) => { if (!condition) throw Error(name); checks.push(name); };
  await page.waitForFunction(() => model.csrf);
  const project = await page.evaluate(async () => {
    const name = `Creation QA ${crypto.randomUUID().slice(0, 8)}`;
    let project;
    for (let i = 1; i <= 60; i++) {
      const result = await api({action:'create', title:`Queued work ${i}`, body:'Existing queue item', labels:[], at_top:false}, project || name);
      project = result.project.id;
    }
    return project;
  });
  const origin = await page.evaluate(() => location.origin);
  await page.goto(`${origin}/#project=${encodeURIComponent(project)}`);
  await page.waitForFunction(project => model.project?.id === project && model.issues.length > 0, project);
  await page.setViewportSize({width:1440, height:1000});
  await page.locator('#new-issue').click();
  check(!await page.locator('#editor-bottom').isChecked(), 'Desktop creation defaults to the front');
  check(await page.locator('#editor-subject').evaluate(el => el === document.activeElement), 'Desktop creation focuses the title for keyboard entry');
  await page.locator('#editor-subject').fill('New companion issue — visible above a long queue');
  await page.screenshot({path:'output/playwright/issue148-desktop-create.png'});
  await page.locator('#editor-submit').click();
  await page.waitForFunction(() => !model.editor && model.issues[0]?.title.startsWith('New companion issue'));
  const number = await page.evaluate(() => model.issues[0].number);
  check(number > 60, 'Fresh number exceeds the existing queue');
  check(await page.locator(`[data-issue-number="${number}"]`).first().isVisible(), 'Creation is visible on the first page');
  check(await page.evaluate(async ({project, number}) => {
    const first = await api({...listOperation(), all:false}, project);
    return first.issues.length === 50 && first.issues[0].number === number;
  }, {project, number}), 'The default 50-item API page starts with the new issue');
  await page.screenshot({path:'output/playwright/issue148-desktop.png'});
  await page.locator(`[data-issue-number="${number}"] .issue-title`).click();
  await page.waitForFunction(number => model.detail?.issue?.number === number || model.detail?.number === number, number);
  const detailHeading = page.getByRole('heading', {name:/New companion issue — visible above a long queue/});
  await detailHeading.waitFor({state:'visible'});
  check(await detailHeading.isVisible(), 'Created issue opens immediately from the companion');
  await page.goto(`${origin}/#project=${encodeURIComponent(project)}`);
  await page.waitForFunction(number => model.issues[0]?.number === number, number);
  checks.push('Front placement survives navigation and reload');
  await page.setViewportSize({width:390, height:844});
  await page.emulateMedia({colorScheme:'light'});
  await page.locator('#quick-issue-open').click();
  await page.waitForFunction(() => document.querySelector('#quick-issue-context').textContent.startsWith('Create in'));
  check(!await page.locator('#quick-issue-bottom').isChecked(), 'Phone Quick Add defaults to the front');
  check(await page.locator('#quick-issue-title').evaluate(el => el === document.activeElement), 'Phone Quick Add focuses the title');
  await page.locator('#quick-issue-title').fill('Phone follow-up from the companion');
  await page.screenshot({path:'output/playwright/issue148-phone-create.png'});
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => !document.querySelector('#quick-issue-dialog').open && model.issues[0]?.title === 'Phone follow-up from the companion');
  check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'Phone list has no horizontal overflow');
  await page.emulateMedia({colorScheme:'dark'});
  await page.screenshot({path:'output/playwright/issue148-phone-dark.png'});
  await page.locator('#new-issue').click();
  await page.locator('#editor-subject').fill('Deliberately queued at the bottom');
  await page.locator('#editor-bottom').check();
  await page.locator('#editor-submit').click();
  await page.waitForFunction(() => !model.editor);
  await page.reload();
  await page.waitForFunction(() => model.issues.at(-1)?.title === 'Deliberately queued at the bottom');
  check(await page.evaluate(() => model.issues[0]?.title === 'Phone follow-up from the companion'), 'Explicit bottom placement persists without displacing the newest issue');
  check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'Phone creation feedback fits the viewport');
  await page.setViewportSize({width:320, height:740});
  check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'The narrow phone layout has no horizontal overflow');
  await page.screenshot({path:'output/playwright/issue148-phone-narrow.png'});
  if (checks.length !== 13) throw Error(`Incomplete browser graph: ${checks.length}/13`);
  return {completed:checks.length, expected:13, checks};
}
