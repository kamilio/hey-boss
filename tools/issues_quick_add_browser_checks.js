async page => {
  const base = 'http://127.0.0.1:4794';
  const checks = [];
  await page.goto(base);
  await page.waitForFunction(() => model.csrf && model.project);
  for (const project of ['poe-code', 'Other']) await page.evaluate(project => api({action:'create', title:'Fixture', body:'', labels:[]}, project), project);
  const current = await page.evaluate(() => model.project.id);
  await page.goto(base+'/#quick-issue=1');
  await page.waitForFunction(() => document.querySelector('#quick-issue-dialog').open && !location.hash.includes('quick-issue'));
  await page.keyboard.press('Escape');
  checks.push('Desktop launch link opens quick add and consumes its flag');
  const open = async () => {
    await page.keyboard.press('Meta+Shift+K');
    await page.waitForFunction(() => document.querySelector('#quick-issue-dialog').open && document.querySelector('#quick-issue-context').textContent.startsWith('Create in'));
    if (await page.locator('#quick-issue-title').evaluate(el => el !== document.activeElement)) throw Error('Input is not focused');
  };
  const create = async (text, title, projectName) => {
    await open();
    await page.locator('#quick-issue-title').fill(text);
    await page.keyboard.press('Enter');
    await page.waitForFunction(() => !document.querySelector('#quick-issue-dialog').open);
    const href = await page.locator('#quick-issue-status a').getAttribute('href');
    const route = await page.evaluate(href => Object.fromEntries(new URLSearchParams(href.split('#')[1])), href);
    const result = await page.evaluate(async ({project,number}) => {
      const boot = await (await fetch('/api/bootstrap')).json();
      return (await (await fetch('/api/action', {method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf},body:JSON.stringify({project,operation:{action:'view',number},host:null})})).json()).issue;
    }, {project:route.project,number:Number(route.issue)});
    if (result.title !== title || result.body !== '' || !route.project.endsWith(projectName)) throw Error('Wrong created issue: '+JSON.stringify(result));
  };
  for (const path of ['/#project='+encodeURIComponent(current), '/#project='+encodeURIComponent(current)+'&view=inbox', '/workers#project='+encodeURIComponent(current), '/mm#project='+encodeURIComponent(current), '/mm?focus=1#project='+encodeURIComponent(current)]) {
    await page.goto(base+path);
    await create('Fix @poe-code from '+path.split('#')[0], 'Fix from '+path.split('#')[0], 'poe-code');
    checks.push('Create from '+path);
  }
  await page.goto(base+'/#project='+encodeURIComponent(current)); await page.waitForFunction(() => model.csrf && model.project);
  await page.locator('#new-issue').click(); await page.locator('#editor-subject').fill('Preserved editor draft');
  await create('Quick while editing', 'Quick while editing', 'hey-boss');
  if (!(await page.locator('#editor-dialog').evaluate(el => el.open)) || await page.locator('#editor-subject').inputValue() !== 'Preserved editor draft') throw Error('Editor disturbed');
  checks.push('Quick creation above another dialog preserves its draft');
  await open(); await page.locator('#quick-issue-title').fill('Fix @missing'); await page.keyboard.press('Enter');
  if (!(await page.locator('#quick-issue-dialog').evaluate(el => el.open)) || !(await page.locator('#quick-issue-error').innerText()).includes('Unknown project')) throw Error('Unknown project accepted');
  await page.locator('#quick-issue-title').fill('Fix @Other @poe-code'); await page.keyboard.press('Enter');
  if (!(await page.locator('#quick-issue-error').innerText()).includes('only one project')) throw Error('Conflicting projects accepted');
  checks.push('Unknown and conflicting projects rejected');
  await page.keyboard.press('Escape');
  if (!(await page.locator('#editor-dialog').evaluate(el => el.open))) throw Error('Escape closed underlying dialog');
  await page.locator('#editor-cancel').click();
  await open(); await page.locator('#quick-issue-title').fill('Cancelled draft'); await page.keyboard.press('Escape'); await open();
  if (await page.locator('#quick-issue-title').inputValue() !== 'Cancelled draft') throw Error('Lost draft');
  checks.push('Escape preserves quick draft');
  await page.locator('#quick-issue-title').fill('IME draft');
  await page.locator('#quick-issue-title').evaluate(el => el.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true,isComposing:true})));
  if (!(await page.locator('#quick-issue-dialog').evaluate(el => el.open))) throw Error('IME Enter submitted');
  checks.push('IME Enter does not submit');
  await page.keyboard.press('Escape');
  await page.setViewportSize({width:390,height:844});
  await page.locator('#quick-issue-open').click();
  await page.waitForFunction(() => document.querySelector('#quick-issue-context').textContent.startsWith('Create in'));
  if (await page.locator('#quick-issue-dialog').evaluate(el => el.getBoundingClientRect().right > innerWidth)) throw Error('Mobile overflow');
  checks.push('Mobile button opens overlay without overflow');
  await page.screenshot({path:'output/playwright/issue14-quick-add-mobile.png'});
  await page.keyboard.press('Escape'); await page.setViewportSize({width:1280,height:800});
  let first = true, requests = [];
  await page.route('**/api/action',async route => {
    const data = route.request().postDataJSON();
    if (data.operation.action !== 'create') return route.continue();
    requests.push(data);
    if (first) { first = false; await route.fetch(); return route.abort('failed'); }
    await page.waitForTimeout(200);
    return route.continue();
  });
  await open(); await page.locator('#quick-issue-title').fill('Retry only once @poe-code'); await page.keyboard.press('Enter');
  await page.waitForFunction(() => !document.querySelector('#quick-issue-error').hidden && !document.querySelector('#quick-issue-title').disabled);
  if (await page.locator('#quick-issue-title').inputValue() !== 'Retry only once @poe-code') throw Error('Lost failed draft');
  await page.keyboard.press('Enter'); await page.keyboard.press('Enter');
  await page.waitForFunction(() => !document.querySelector('#quick-issue-dialog').open);
  if (requests.length !== 2 || requests[0].request_id !== requests[1].request_id) throw Error('Retries not deduplicated');
  checks.push('Lost response retry uses the same request ID and blocks double Enter');
  await page.unroute('**/api/action');
  await open(); await page.locator('#quick-issue-title').fill('Fix @"Other"');
  await page.screenshot({path:'output/playwright/issue14-quick-add-desktop.png'});
  return {passed:checks.length,checks};
}
