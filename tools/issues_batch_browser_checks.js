// Run with playwright-cli run-code --filename on an isolated issue web server.
// Fixture: named project "Batch QA", two ordinary issues with the label "keep".
async page => {
  const checks = [], errors = [], engine = page.context().browser().browserType().name();
  const check = (ok, message) => { if (!ok) throw Error(message); checks.push(message); };
  const root = page.url().split('#')[0] + '#project=named%3ABatch%20QA';
  page.on('pageerror', e => errors.push(e.message));
  await page.goto(root);
  await page.reload();
  await page.locator('.issue-row').nth(1).waitFor();
  const mutate = async (assignment, add, remove, dry_run = false, stale = false) => page.evaluate(async args => {
    const issues = (await api({action:'list',state:'open',mine:false,unassigned:false,labels:[],search:null,limit:100,offset:0})).issues;
    const edits = issues.map((i, index) => ({number:i.number,if_version:i.version + (args.stale && index === 1 ? 1 : 0),expected_assignee:i.assignee,add_labels:[args.add],remove_labels:[args.remove],assignment:args.assignment}));
    return api({action:'batch',edits,dry_run:args.dry_run}, model.project.id, args.dry_run ? null : crypto.randomUUID());
  }, {assignment,add,remove,dry_run,stale});
  const preview = await mutate('boss','PR ready','rework needed',true);
  check(preview.accepted && !preview.applied && preview.results.every(r => r.status === 'would_change'), 'Preview reports coupled changes without applying');
  await page.reload();
  await page.locator('.issue-row').nth(1).waitFor();
  check(!(await page.locator('#issue-list').innerText()).includes('PR ready'), 'Preview leaves list labels untouched');
  const rejected = await mutate('boss','PR ready','rework needed',false,true);
  check(!rejected.accepted && rejected.results[0].status === 'blocked' && rejected.results[1].status === 'rejected', 'Stale group blocks the other issue');
  const fits = () => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth);
  for (const phase of ['ready','rework']) {
    const label = phase === 'ready' ? 'PR ready' : 'rework needed';
    const applied = await mutate(phase === 'ready' ? 'boss' : 'unassign', label, phase === 'ready' ? 'rework needed' : 'PR ready');
    check(applied.applied && applied.results.every(r => r.status === 'changed'), `${phase}: both issues updated`);
    for (const theme of ['light','dark']) {
      await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
      for (const width of [1440,768,390,320]) {
        await page.setViewportSize({width,height:1000});
        await page.goto(root);
        await page.reload();
        await page.locator('.issue-row').nth(1).waitFor();
        const rows = await page.locator('.issue-row').allTextContents();
        check(rows.length === 2 && rows.every(r => r.includes(label) && r.includes('keep')), `${phase}/${theme}/${width}: labels preserved in list`);
        check(await fits(), `${phase}/${theme}/${width}: list fits`);
        await page.screenshot({path:`output/playwright/issue56/${engine}-${phase}-${theme}-${width}-list.png`});
        await page.locator('.issue-title').first().click();
        await page.locator('.assignee-line').waitFor();
        check((await page.locator('.assignee-line').innerText()).includes(phase === 'ready' ? 'Boss' : 'Unassigned'), `${phase}/${theme}/${width}: owner visible`);
        check(await page.locator('.state-pill.open').innerText() === 'Open', 'Handoff leaves issue open');
        await page.getByRole('button',{name:'View activity',exact:true}).click();
        await page.locator('.timeline-item').filter({hasText:'updated labels or ownership'}).first().waitFor();
        const changes = page.locator('.timeline-item').filter({hasText:'updated labels or ownership'}).last();
        await changes.getByText('View changes',{exact:true}).click();
        check((await changes.locator('pre').innerText()).includes(label), `${phase}: readable audit contains labels`);
        check(await fits(), `${phase}/${theme}/${width}: expanded activity fits`);
        await changes.scrollIntoViewIfNeeded();
        await page.screenshot({path:`output/playwright/issue56/${engine}-${phase}-${theme}-${width}-activity.png`});
        await page.locator('.assignee-line').scrollIntoViewIfNeeded();
        await page.screenshot({path:`output/playwright/issue56/${engine}-${phase}-${theme}-${width}-owner.png`});
      }
    }
  }
  check(errors.length === 0, 'No browser runtime errors: ' + errors.join('; '));
  return {passed:checks.length,engine};
}
