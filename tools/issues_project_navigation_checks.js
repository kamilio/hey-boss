// Run against an isolated issue web server; exercise actual links and remembered context.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  const origin = await page.evaluate(() => location.origin);
  const boot = await (await page.request.get(origin + '/api/bootstrap')).json();
  const projects = ['named:Navigation Alpha', 'named:Navigation Beta & more'];
  const errorTitle = project => project.replace(/^named:/, '') + ' failure';
  for (const project of projects) {
    const response = await page.request.post(origin + '/api/action', {
      headers: {'X-Hey-Boss-CSRF': boot.csrf, Origin:origin},
      data: {project, operation: {action:'project_init',settings:{prs_enabled:true,worktree_enabled:false,if_version:0}}, request_id:null},
    });
    check(response.ok(), 'Create synthetic project ' + project + (response.ok() ? '' : ': ' + await response.text()));
    for (const data of [
      {action:'enable',project,enabled:true},
      {action:'ingest',project,events:[{source:'navigation-test',event_id:project,timestamp:Date.now(),message:errorTitle(project),count:1}]},
    ]) {
      const seeded = await page.request.post(origin + '/api/errors', {headers:{'X-Hey-Boss-CSRF':boot.csrf,Origin:origin},data});
      check(seeded.ok(), 'Seed ' + data.action + ' for ' + project);
    }
  }
  const selected = async project => {
    await page.waitForFunction(name => document.querySelector('#project-name')?.textContent === name, project.replace(/^named:/, ''));
  };
  await page.goto(origin + '/#project=' + encodeURIComponent(projects[0]));
  await selected(projects[0]);
  await page.locator('#nav-artifacts').click();
  await selected(projects[0]);
  await page.locator('#project-trigger').click();
  await page.locator('[data-project="' + projects[1] + '"]').click();
  await selected(projects[1]);
  await page.locator('#nav-inbox').click();
  await page.waitForFunction(() => !document.querySelector('#inbox-view').hidden);
  await page.locator('#nav-issues').click();
  await selected(projects[1]);
  check(true, 'Artifacts → Inbox → Issues preserves selection');
  for (const selector of ['#nav-workers', '#nav-artifacts', '#nav-issues']) {
    if(selector==='#nav-workers')await page.locator('#navigation-more').click();
    await page.locator(selector).click();
    await selected(projects[1]);
    check(true, selector + ' preserves selection');
  }
  for (const path of ['/artifacts', '/workers', '/']) {
    await page.goto(origin + path);
    await selected(projects[1]);
    await page.reload();
    await selected(projects[1]);
    check(true, path + ' restores selection without a project URL');
  }
  await page.goto(origin + '/errors');
  await selected(projects[1]);
  await page.waitForFunction(() => !document.querySelector('#errors-status').textContent.startsWith('Loading'));
  check(await page.locator('#errors-alert').isHidden(), 'Errors loads the remembered project');
  const errorsFor = async project => {
    await page.locator('#error-rows').getByRole('button', {name:errorTitle(project),exact:true}).waitFor();
    check(await page.locator('#error-rows .issue-row').count() === 1, 'Error list belongs only to ' + project);
  };
  await errorsFor(projects[1]);
  await page.locator('#project-trigger').click();
  await page.locator('[data-project="' + projects[0] + '"]').click();
  await selected(projects[0]);
  await errorsFor(projects[0]);
  await page.goBack();
  await selected(projects[1]);
  await errorsFor(projects[1]);
  await page.goForward();
  await selected(projects[0]);
  await errorsFor(projects[0]);
  check(true, 'Errors picker follows browser Back and Forward');
  const sections = ['/errors', '/jobs', '/artifacts', '/merged-prs', '/skills', '/admin', '/secrets'];
  for (const path of sections) {
    await page.goto(origin + path + '#project=' + encodeURIComponent(projects[0]));
    await selected(projects[0]);
    await page.locator('#project-trigger').click();
    await page.locator('[data-project="' + projects[1] + '"]').click();
    await selected(projects[1]);
    await page.reload();
    await selected(projects[1]);
    await page.goto(origin + path);
    await selected(projects[1]);
    await page.locator('#nav-errors').click();
    await selected(projects[1]);
    await page.locator('#nav-issues').click();
    await selected(projects[1]);
    check(true, path + ' keeps picker selection on reload, direct entry and links through Errors');
  }
  await page.goto(origin + '/workers#project=' + encodeURIComponent(projects[0]));
  await selected(projects[0]);
  await page.locator('#project-trigger').click();
  await page.locator('[data-project="' + projects[1] + '"]').click();
  await selected(projects[1]);
  await page.locator('#nav-inbox').click();
  await page.waitForFunction(() => !document.querySelector('#inbox-view').hidden);
  await page.locator('#nav-artifacts').click();
  await selected(projects[1]);
  check(true, 'Workers picker → Inbox → Artifacts preserves selection');
  await page.goto(origin + '/artifacts#project=' + encodeURIComponent(projects[0]));
  await selected(projects[0]);
  await page.goto(origin + '/');
  await selected(projects[0]);
  check(true, 'Explicit artifact link overrides remembered selection');
  await page.addInitScript(() => {
    Storage.prototype.getItem = Storage.prototype.setItem = () => { throw Error('Storage unavailable'); };
  });
  await page.goto(origin + '/artifacts#project=' + encodeURIComponent(projects[1]));
  await selected(projects[1]);
  check(await page.locator('#nav-workers').getAttribute('href')==='/workers','Global worker link is independent of storage and project');
  await page.locator('#nav-issues').click();
  await selected(projects[1]);
  await page.locator('#nav-errors').click();
  await selected(projects[1]);
  await page.reload();
  await selected(projects[1]);
  await page.locator('#nav-artifacts').click();
  await selected(projects[1]);
  check(true, 'URL navigation works when browser storage is unavailable');
  check(!errors.length, 'No JavaScript errors: ' + errors.join('; '));
  return {passed:checks.length, checks};
}
