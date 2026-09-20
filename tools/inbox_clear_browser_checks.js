async (page) => {
  const checks = [], audits = [], errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const check = (value, name) => { if (!value) throw Error(name); checks.push(name); };
  const desktop = await page.locator('#inbox-view').count() > 0;
  const prefix = `output/playwright/issue46/${desktop ? 'desktop' : 'phone'}-${page.context().browser().browserType().name()}`;
  const clear = () => page.getByRole('button', { name: 'Clear all', exact: true }).first();
  const dialog = () => page.getByRole('dialog').filter({ has: page.getByRole('heading', { name: 'Clear all unread notices?' }) });
  const confirm = () => dialog().getByRole('button', { name: 'Clear all', exact: true });
  const pending = () => page.evaluate(async desktop => {
    const data = desktop ? await inboxApi({action:'list'}) : await (await fetch('/api/tasks')).json();
    return data.tasks.filter(task => task.status === 'pending').length;
  }, desktop);
  const audit = async name => {
    await page.evaluate(/* AXE_SOURCE */);
    const violations = await page.evaluate(async () => (await axe.run(document, {runOnly:{type:'tag',values:['wcag2a','wcag2aa','wcag21aa','best-practice']}})).violations.map(v=>({id:v.id,nodes:v.nodes.map(n=>n.target)})));
    audits.push({ name, violations });
    check(violations.length === 0, name + ' has no accessibility violations: ' + JSON.stringify(violations));
  };
  const geometry = async name => {
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), name + ' has no horizontal overflow');
    check(await clear().evaluate(el => { const r=el.getBoundingClientRect();return r.width>70&&r.height>=36&&r.left>=0&&r.right<=innerWidth; }), name + ' clear button fits and has a usable target');
    if (await dialog().count()) check(await dialog().evaluate(el => {const r=el.getBoundingClientRect();return r.left>=0&&r.right<=innerWidth&&r.top>=0&&r.bottom<=innerHeight;}), name + ' dialog fits viewport');
  };
  await clear().waitFor();
  const initial = await pending();
  check(initial === 5, 'fixture starts with five unread notices');
  for (const [width,height,scheme] of [[1440,1000,'light'],[1440,1000,'dark'],[390,844,'light'],[390,844,'dark'],[320,740,'dark']]) {
    await page.setViewportSize({width,height});await page.emulateMedia({colorScheme:scheme,reducedMotion:'reduce'});
    await geometry(`${width}-${scheme} list`);await audit(`${width}-${scheme} list`);
    await page.screenshot({path:prefix+`-${width}-${scheme}.png`});
    await clear().click();await dialog().waitFor();
    const message = await dialog().innerText();
    check(message.includes('without an answer or approval')&&message.includes('History will be kept'), 'confirmation explains cancellation and preserved history');
    await geometry(`${width}-${scheme} confirmation`);await audit(`${width}-${scheme} confirmation`);
    await page.screenshot({path:prefix+`-${width}-${scheme}-confirmation.png`});
    await page.keyboard.press('Escape');await dialog().waitFor({state:'hidden'});
    check(await pending() === initial, 'Escape preserves every unread notice');
    check(await clear().evaluate(el=>document.activeElement===el), 'Escape restores focus to Clear all');
  }
  if (desktop) {
    await page.locator('#inbox-project-filter').selectOption('Inbox QA');
    await page.waitForFunction(()=>model.route.inbox_project==='Inbox QA');
    await clear().click();await dialog().waitFor();
    check((await dialog().innerText()).includes('5 notices')&&(await dialog().innerText()).includes('hidden by filters'), 'filtered Inbox confirms clearing across all projects');
    await page.locator('#confirm-cancel').click();
    await page.locator('#inbox-search').fill('no results');await page.waitForFunction(()=>model.route.inbox_search==='no results');
    check(await clear().isEnabled(),'Clear all remains available when filters hide unread notices');
    await page.locator('#inbox-search').fill('');await page.waitForFunction(()=>model.route.inbox_search==='');
  }
  const endpoint = desktop ? '**/api/inbox' : '**/api/tasks/clear';
  await page.route(endpoint, async route => {
    const action = route.request().postDataJSON();
    if (!desktop || action.action === 'clear') await route.fulfill({status:503,contentType:'application/json',body:JSON.stringify(desktop?{ok:false,error:{message:'Clear unavailable. Try again.'}}:{error:'Clear unavailable. Try again.'})});
    else await route.continue();
  });
  await clear().click();await confirm().click();
  await page.getByText('Clear unavailable. Try again.',{exact:true}).waitFor();
  check(await pending()===initial,'failed clear preserves unread notices');
  if(!desktop){await page.keyboard.press('Escape');await dialog().waitFor({state:'hidden'});}
  check(await clear().isEnabled(),'failed clear leaves a working retry button');
  await page.unrouteAll({behavior:"wait"});
  let release;const gate=new Promise(resolve=>release=resolve);let submissions=0;
  await page.route(endpoint, async route => {
    const action=route.request().postDataJSON();
    if(desktop&&action.action!=='clear'){await route.continue();return;}
    submissions++;const response=await route.fetch();await gate;await route.fulfill({response});
  });
  await clear().click();await confirm().click();
  await page.getByRole('button',{name:'Clearing…',exact:true}).waitFor();
  check(await page.getByRole('button',{name:'Clearing…',exact:true}).isDisabled(),'clearing disables duplicate submissions');
  if(!desktop){await page.keyboard.press('Escape');check(await dialog().isVisible(),'busy confirmation remains open on Escape');}
  release();
  await page.waitForFunction(desktop=>desktop?document.querySelector('[data-inbox-clear-all]')?.disabled&&document.querySelector('[data-inbox-clear-all]')?.textContent.includes('Clear all'):document.querySelector('.clear-inbox-button')?.disabled&&document.querySelector('.page-title .count')?.textContent==='0',desktop);
  await page.unrouteAll({behavior:'wait'});
  check(await pending()===0,'confirmed Clear all empties the unread Inbox');
  check(submissions===1,'clear sends exactly one browser batch request');
  check(await clear().isDisabled(),'empty Inbox disables Clear all');
  await page.screenshot({path:prefix+'-empty.png'});
  await audit('empty Inbox');
  if(desktop)await page.locator('[data-inbox-state="archive"]').click();
  else await page.getByRole('button',{name:'Activity',exact:true}).click();
  await page.getByText('Publish release?',{exact:true}).waitFor();
  check(await page.getByRole('button',{name:'Clear all',exact:true}).count()===0,'Activity does not offer a destructive clear action');
  const statuses=await page.evaluate(async desktop=>{
    const value=desktop?await inboxApi({action:'list'}):await(await fetch('/api/tasks')).json();
    return Object.fromEntries(value.tasks.map(task=>[task.taskID,task.status]));
  },desktop);
  check(statuses['notice-1']==='ok'&&statuses['notice-5']==='ok','updates and alerts become read');
  check(['notice-2','notice-3','notice-4'].every(id=>statuses[id]==='cancelled'),'questions and reviews become cancelled');
  check(Object.keys(statuses).length===6,'all history records remain available');
  await page.screenshot({path:prefix+'-activity.png'});await audit('Activity after clear');
  await page.reload();
  await page.waitForFunction(async desktop=>{const v=desktop?await inboxApi({action:'list'}):await(await fetch('/api/tasks')).json();return v.tasks.every(t=>t.status!=='pending');},desktop);
  check(await pending()===0,'clear survives reload');
  check(errors.length===0,'no uncaught browser errors: '+errors.join(', '));
  return {passed:checks.length,checks,audits};
}
