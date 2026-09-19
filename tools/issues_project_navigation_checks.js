// Run against an isolated issue web server; exercise actual links and remembered context.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  const origin = await page.evaluate(() => location.origin);
  const boot = await (await page.request.get(origin + '/api/bootstrap')).json();
  const projects = ['named:Navigation Alpha', 'named:Navigation Beta & more'];
  for (const project of projects) {
    const response = await page.request.post(origin + '/api/action', {
      headers: {'X-Hey-Boss-CSRF': boot.csrf},
      data: {project, operation: {action:'list',state:'open',mine:false,unassigned:false,labels:[],search:null,limit:50,offset:0}, request_id:null},
    });
    check(response.ok(), 'Create synthetic project ' + project);
  }
  const selected = async project => {
    await page.waitForFunction(name => document.querySelector('#project-name')?.textContent === name, project.replace(/^named:/, ''));
  };
  await page.goto(origin + '/#project=' + encodeURIComponent(projects[0]));
  await selected(projects[0]);
  await page.locator('#nav-mindmaps').click();
  await selected(projects[0]);
  await page.locator('#project-trigger').click();
  await page.locator('[data-project="' + projects[1] + '"]').click();
  await selected(projects[1]);
  await page.locator('#nav-inbox').click();
  await page.waitForFunction(() => !document.querySelector('#inbox-view').hidden);
  await page.locator('#nav-issues').click();
  await selected(projects[1]);
  check(true, 'Mindmaps → Inbox → Issues preserves selection');
  for (const selector of ['#nav-workers', '#nav-mindmaps', '#nav-issues']) {
    await page.locator(selector).click();
    await selected(projects[1]);
    check(true, selector + ' preserves selection');
  }
  for (const path of ['/mm', '/workers', '/']) {
    await page.goto(origin + path);
    await selected(projects[1]);
    await page.reload();
    await selected(projects[1]);
    check(true, path + ' restores selection without a project URL');
  }
  await page.goto(origin + '/workers#project=' + encodeURIComponent(projects[0]));
  await selected(projects[0]);
  await page.locator('#project-trigger').click();
  await page.locator('[data-project="' + projects[1] + '"]').click();
  await selected(projects[1]);
  await page.locator('#nav-inbox').click();
  await page.waitForFunction(() => !document.querySelector('#inbox-view').hidden);
  await page.locator('#nav-mindmaps').click();
  await selected(projects[1]);
  check(true, 'Workers picker → Inbox → Mindmaps preserves selection');
  await page.goto(origin + '/mm#project=' + encodeURIComponent(projects[0]));
  await selected(projects[0]);
  await page.goto(origin + '/');
  await selected(projects[0]);
  check(true, 'Explicit mindmap link overrides remembered selection');
  await page.addInitScript(() => {
    Storage.prototype.getItem = Storage.prototype.setItem = () => { throw Error('Storage unavailable'); };
  });
  await page.goto(origin + '/mm#project=' + encodeURIComponent(projects[1]));
  await selected(projects[1]);
  await page.locator('#nav-workers').click();
  await selected(projects[1]);
  await page.locator('#nav-issues').click();
  await selected(projects[1]);
  check(true, 'URL navigation works when browser storage is unavailable');
  check(!errors.length, 'No JavaScript errors: ' + errors.join('; '));
  return {passed:checks.length, checks};
}
