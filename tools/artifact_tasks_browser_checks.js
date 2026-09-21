async page => {
  const checks = [], errors = [], requests = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  const base = 'http://127.0.0.1:4788/#project=named%3AArtifact%20Tasks';
  const goto = url => page.goto(url,{waitUntil:'domcontentloaded'});
  page.setDefaultTimeout(90000);
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => {
    if (request.url().endsWith('/api/action')) requests.push(request.postDataJSON());
  });
  const labels = async number => page.evaluate(async number => {
    const bootstrap = await fetch('/api/bootstrap').then(r => r.json());
    const value = await fetch('/api/action', {method:'POST', headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':bootstrap.csrf}, body:JSON.stringify({project:'named:Artifact Tasks',operation:{action:'view',number}})}).then(r => r.json());
    return value.issue.labels;
  }, number);
  await page.unrouteAll({behavior:'wait'});
  await goto(base); await page.evaluate(() => localStorage.clear());
  await page.locator('[data-issue-number="1"]').waitFor();
  await page.evaluate(async () => {
    const bootstrap = await fetch('/api/bootstrap').then(r => r.json());
    if (bootstrap.project.id !== 'named:Artifact Tasks') throw Error('Refusing to modify a non-fixture project');
    const action = async operation => {
      const value = await fetch('/api/action',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':bootstrap.csrf},body:JSON.stringify({project:'named:Artifact Tasks',operation,...(operation.action === 'delete' ? {request_id:crypto.randomUUID()} : {})})}).then(r => r.json());
      if (!value.ok) throw Error(value.error?.message); return value;
    };
    const seed = await action({action:'view',number:1});
    if (seed.issue.title !== 'Explore task intent') throw Error('Refusing to modify non-fixture issues');
    const list = await action({action:'list',state:'all',mine:false,unassigned:false,assignee:null,labels:[],search:null,limit:50,offset:0,all:true});
    for (const issue of list.issues) {
      if (/^(Visual |Quick research task)/.test(issue.title) && !issue.deleted_at) await action({action:'delete',number:issue.number,force:false});
    }
  });
  for (const kind of ['implement','plan','research']) {
    await page.locator('#new-issue').click();
    check(await page.locator('#editor-kind').inputValue() === 'implement', 'New editor defaults to Implement');
    await page.locator('#editor-kind').selectOption(kind);
    await page.locator('#editor-subject').fill(`Visual ${kind} task`);
    await page.locator('#editor-body').fill('Keep the exact requirements.\n\n## Evidence\nReview sources.');
    await page.locator('#editor-labels').fill('ready, design');
    await page.locator('#editor-submit').click();
    await page.locator('#editor-dialog').waitFor({state:'hidden'});
    await page.getByRole('link',{name:`Visual ${kind} task`,exact:true}).waitFor();
    const operation = requests.filter(r => r.operation.action === 'create').at(-1).operation;
    check(JSON.stringify(operation.labels.sort()) === JSON.stringify((kind === 'implement' ? ['design','ready'] : ['design','ready',`task:${kind}`]).sort()), `${kind}: creation preserves labels and task intent`);
  }
  await page.getByRole('link',{name:'Visual research task',exact:true}).click();
  await page.locator('[data-edit]').click();
  check(await page.locator('#editor-kind').inputValue() === 'research', 'Editing restores task intent');
  check(!await page.locator('#editor-tags .tag-chips').textContent().then(s => s.includes('task:')), 'Task metadata stays out of editable tags');
  await page.locator('#editor-kind').selectOption('plan');
  await page.locator('#editor-cancel').click();
  await page.locator('[data-edit]').click();
  check(await page.locator('#editor-kind').inputValue() === 'plan', 'Unsaved task selection survives closing and reopening');
  await page.locator('#editor-submit').click(); await page.locator('#editor-dialog').waitFor({state:'hidden'});
  const number = await page.evaluate(() => Number(new URLSearchParams(location.hash.slice(1)).get('issue')));
  check(JSON.stringify((await labels(number)).sort()) === JSON.stringify(['design','ready','task:plan'].sort()), 'Switching task replaces metadata and preserves unrelated tags');
  await page.locator('[data-edit]').click(); await page.locator('#editor-kind').selectOption('implement');
  await page.locator('#editor-submit').click(); await page.locator('#editor-dialog').waitFor({state:'hidden'});
  check(JSON.stringify((await labels(number)).sort()) === JSON.stringify(['design','ready'].sort()), 'Switching back to Implement removes task metadata');
  await page.locator('[data-edit]').click();
  check((await page.locator('#editor-body').inputValue()).includes('Keep the exact requirements.'), 'Task edits preserve the description');
  await page.locator('#editor-cancel').click();

  await page.locator('#quick-issue-open').click();
  await page.locator('#quick-issue-title').fill('Quick research task');
  await page.locator('#quick-issue-kind').selectOption('research');
  let reject = true;
  await page.route('**/api/action', route => {
    if (reject && route.request().postDataJSON()?.operation.action === 'create') {
      reject = false; return route.fulfill({status:503,json:{ok:false,error:{message:'Synthetic connection interruption'}}});
    }
    return route.continue();
  });
  await page.locator('#quick-issue-submit').click(); await page.locator('#quick-issue-error').waitFor();
  check(await page.locator('#quick-issue-kind').inputValue() === 'research', 'Quick Add preserves task selection after failure');
  check(await page.locator('#quick-issue-title').inputValue() === 'Quick research task', 'Quick Add preserves the title after failure');
  await page.locator('#quick-issue-submit').click(); await page.locator('#quick-issue-dialog').waitFor({state:'hidden'});
  const retries = requests.filter(r => r.operation.action === 'create' && r.operation.title === 'Quick research task');
  check(retries.length === 2 && retries[0].request_id === retries[1].request_id, 'Quick Add safely reuses the request ID');
  check(retries.every(r => JSON.stringify(r.operation.labels) === '["task:research"]'), 'Quick Add sends artifact task metadata on every retry');
  await page.unroute('**/api/action');
  await page.locator('#quick-issue-open').click();
  check(await page.locator('#quick-issue-kind').inputValue() === 'implement', 'Quick Add resets task choice after successful creation');
  await page.locator('#quick-issue-close').click();

  await goto(base); await page.getByRole('link',{name:'Visual plan task',exact:true}).waitFor();
  for (const theme of ['light','dark']) {
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:width <= 390 ? 844 : 1000});
      await page.getByRole('link',{name:'Visual plan task',exact:true}).waitFor();
      check(await page.locator('.task-kind-badge').count() >= 2, `${theme}/${width}: quiet badges identify artifact tasks`);
      await page.screenshot({path:`output/playwright/issue78/${theme}-${width}-list.png`});
      await page.locator('#new-issue').click(); await page.locator('#editor-kind').selectOption('research');
      await page.locator('#editor-subject').fill('Research task with a longer title');
      await page.locator('#editor-kind').focus();
      if (width <= 390) check(await page.locator('#editor-kind').evaluate(el => parseFloat(getComputedStyle(el).fontSize) >= 16), `${theme}/${width}: editor task text avoids phone focus zoom`);
      await page.locator('.task-kind-row').evaluate(el => el.scrollIntoView({block:'nearest'}));
      check(await page.locator('#editor-kind').evaluate(el => document.activeElement === el && !!el.labels.length && !!document.getElementById(el.getAttribute('aria-describedby'))), `${theme}/${width}: task control has keyboard focus and accessible help`);
      check(await page.locator('#editor-dialog').evaluate(el => el.getBoundingClientRect().left >= 0 && el.getBoundingClientRect().right <= innerWidth && el.scrollWidth <= el.clientWidth + 1), `${theme}/${width}: editor fits`);
      check(await page.locator('#editor-kind-help').textContent() === 'Findings and sources, saved as linked artifacts.', `${theme}/${width}: contextual task help`);
      await page.screenshot({path:`output/playwright/issue78/${theme}-${width}-editor.png`});
      await page.keyboard.press('Escape');
      check(!await page.locator('#editor-dialog').isVisible(), `${theme}/${width}: keyboard dismisses editor`);
      await page.locator('#quick-issue-open').click(); await page.locator('#quick-issue-kind').selectOption('plan');
      if (width <= 390) check(await page.locator('#quick-issue-kind').evaluate(el => parseFloat(getComputedStyle(el).fontSize) >= 16), `${theme}/${width}: Quick Add task text avoids phone focus zoom`);
      check(await page.locator('#quick-issue-dialog').evaluate(el => el.getBoundingClientRect().left >= 0 && el.getBoundingClientRect().right <= innerWidth && el.scrollWidth <= el.clientWidth + 1), `${theme}/${width}: Quick Add fits`);
      await page.screenshot({path:`output/playwright/issue78/${theme}-${width}-quick.png`});
      await page.locator('#quick-issue-close').click();
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${theme}/${width}: page has no horizontal overflow`);
    }
  }
  check(errors.length === 0, `No browser errors: ${errors}`);
  return {checks:checks.length,errors};
}
