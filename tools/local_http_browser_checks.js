// Run with playwright-cli run-code --filename after opening the real private fixture.
async page => {
  const port = await page.evaluate(() => location.port);
  const alias = `http://hey-boss.test:${port}`;
  const numeric = `http://127.0.0.1:${port}`;
  const hash = '#project=named%3ALocal%20HTTP%20QA&view=issues&inbox_state=unread&state=open&owner=all';
  const checks = [];
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const check = (condition, description) => {
    if (!condition) throw new Error(description);
    checks.push(description);
  };
  const ready = () => page.waitForFunction(() => model.csrf && model.project && model.signature);
  async function create(title) {
    await page.getByRole('button', {name:'New issue N', exact:true}).click();
    await page.getByRole('textbox', {name:'Title', exact:true}).fill(title);
    await page.getByRole('textbox', {name:'Issue description', exact:true}).fill('Disposable browser verification of shared state, ordinary HTTP, and preserved deep links.');
    await page.getByRole('button', {name:'Create issue', exact:true}).click();
    await page.locator('dialog[open]').waitFor({state:'hidden'});
  }
  await page.setViewportSize({width:1440,height:1000});
  await page.emulateMedia({colorScheme:'light'});
  await page.goto(alias + '/' + hash);
  await ready();
  check(await page.evaluate(() => !isSecureContext && !crypto.subtle && !crypto.randomUUID), 'Hostname tested without secure-context exceptions');
  await create('Written through hey-boss.test');
  await page.goto(numeric + '/' + hash);
  await ready();
  await page.getByRole('link', {name:'Written through hey-boss.test', exact:true}).waitFor();
  check(true, 'Hostname UI write is visible through numeric loopback');
  await create('Written through numeric loopback');
  await page.goto(alias + '/' + hash);
  await ready();
  await page.getByRole('link', {name:'Written through numeric loopback', exact:true}).waitFor();
  check(true, 'Numeric UI write is visible through hostname');
  check(await page.evaluate(() => {const p=new URLSearchParams(location.hash.slice(1));return p.get('project')==='named:Local HTTP QA' && p.get('view')==='issues' && p.get('inbox_state')==='unread' && p.get('state')==='open' && p.get('owner')==='all';}), 'Project/view/filter fragment survives hostname navigation');
  await page.screenshot({path:'output/playwright/issue71/desktop-list.png'});
  await page.getByRole('link', {name:'Written through numeric loopback', exact:true}).click();
  await page.getByRole('heading', {name:'Written through numeric loopback'}).waitFor();
  const deepLink = page.url();
  await page.reload();
  await page.getByRole('heading', {name:'Written through numeric loopback'}).waitFor();
  check(page.url() === deepLink, 'Issue deep link survives browser reload');
  await page.screenshot({path:'output/playwright/issue71/desktop-detail.png'});
  const security = await page.evaluate(async () => {
    const boot = await (await fetch('/api/bootstrap')).json();
    const operation = {action:'create',title:'Must be rejected',body:'',labels:[]};
    const response = await fetch('/api/action', {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({project:boot.project.id,operation})});
    return response.status;
  });
  check(security === 403, 'Real browser write without CSRF is rejected');
  for (const [width,height,scheme] of [[390,844,'light'],[390,844,'dark'],[320,720,'light']]) {
    await page.setViewportSize({width,height});
    await page.emulateMedia({colorScheme:scheme});
    await page.goto(alias + '/' + hash);
    await ready();
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${width}px ${scheme} list has no horizontal overflow`);
    await page.screenshot({path:`output/playwright/issue71/mobile-${width}-${scheme}.png`});
    await page.getByRole('link', {name:'Written through hey-boss.test', exact:true}).click();
    await page.getByRole('heading', {name:'Written through hey-boss.test'}).waitFor();
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${width}px ${scheme} detail has no horizontal overflow`);
    await page.screenshot({path:`output/playwright/issue71/detail-${width}-${scheme}.png`});
  }
  await page.setViewportSize({width:1440,height:1000});
  await page.goto(alias + '/mm?focus=1' + hash);
  await page.waitForFunction(() => document.documentElement.classList.contains('mindmap-focus'));
  check(await page.evaluate(() => location.pathname === '/mm' && location.search === '?focus=1'), 'Mindmap path and query preserved at hostname');
  check(errors.length === 0, `No JavaScript page errors: ${errors.join('; ')}`);
  return {complete:true,completed:checks.length,expected:14,checks};
}
