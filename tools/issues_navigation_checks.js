// Exercise immediate route changes while list responses are deliberately delayed.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('dialog', dialog => dialog.accept().catch(() => {}));
  page.on('pageerror', error => errors.push(error.message));
  await page.reload();
  await page.waitForFunction(() => model.csrf && model.project);
  const origin = await page.evaluate(() => location.origin);
  const project = await page.evaluate(async () => (await api({action:'create',title:'Navigation fixture',body:'Markdown',labels:[]},'Navigation '+crypto.randomUUID())).project.id);
  const detailURL = origin+'/#project='+encodeURIComponent(project)+'&issue=1';
  const detail = async () => {
    await page.goto(detailURL);
    await page.waitForFunction(project => model.project?.id===project && model.detail?.issue.number===1, project);
  };
  await detail();
  await page.locator('#comment-body').fill('Comment survives fast navigation');
  let release;
  let heldLists = 0;
  const pendingLists = [];
  const listGate = new Promise(resolve => { release = resolve; });
  await page.route('**/api/action', async route => {
    const payload = route.request().postDataJSON();
    if ((payload.operation?.action || payload.action) === 'list') {
      heldLists++;
      let finish;
      pendingLists.push(new Promise(resolve => { finish = resolve; }));
      try {
        await listGate;
        await route.continue();
      } finally { finish(); }
      return;
    }
    await route.continue();
  });
  try {
    const immediate = await page.evaluate(() => {
      const sequence = model.sequence;
      document.querySelector('[data-back]').click();
      document.body.focus();
      document.body.dispatchEvent(new KeyboardEvent('keydown',{key:'/',bubbles:true,cancelable:true}));
      return {issue:model.route.issue,view:!document.querySelector('#list-view').hidden,focused:document.activeElement.id,sequence:model.sequence-sequence};
    });
    check(immediate.issue===null && immediate.view, 'Back updates route and list synchronously');
    check(immediate.focused==='issue-search', 'Slash works in the same event turn as Back');
    check(immediate.sequence===1, 'Navigation starts exactly one route render');
    await page.waitForFunction(()=>model.route.issue===null);
    await page.locator('#new-issue').focus();
    await page.keyboard.press('n');
    await page.locator('#editor-subject').fill('Draft survives pending list');
    await page.keyboard.press('Escape');
    await page.keyboard.press('n');
    check(await page.locator('#editor-subject').inputValue()==='Draft survives pending list', 'New issue draft restores while list request is pending');
    await page.keyboard.press('Escape');
    await page.keyboard.press('/');
    check(await page.locator('#issue-search').evaluate(el=>el===document.activeElement), 'Real keyboard Slash works before list response');
    check(heldLists>0 && !await page.evaluate(()=>!!model.signature), 'List response was actually held during shortcuts');
  } finally {
    release();
    await Promise.all(pendingLists);
    await page.unroute('**/api/action');
  }
  await page.waitForFunction(()=>model.signature);
  await page.goBack();
  await page.waitForFunction(()=>model.route.issue===1 && model.detail?.issue.number===1);
  check(await page.locator('#comment-body').inputValue()==='Comment survives fast navigation', 'Browser Back restores comment draft');
  await page.goForward();
  await page.waitForFunction(()=>model.route.issue===null && !document.querySelector('#list-view').hidden);
  check(true, 'Browser Forward restores list');
  for (let i=0;i<20;i++) {
    await detail();
    await page.locator('[data-back]').click();
    await page.locator('#new-issue').focus();
    await page.keyboard.press('n');
    await page.locator('#editor-subject').fill('Rapid draft '+i);
    await page.keyboard.press('Escape');
    await page.keyboard.press('n');
    check(await page.locator('#editor-subject').inputValue()==='Rapid draft '+i, 'Rapid cycle '+i+' preserves editor draft');
    await page.keyboard.press('Escape');
    await page.keyboard.press('/');
    check(await page.locator('#issue-search').evaluate(el=>el===document.activeElement), 'Rapid cycle '+i+' focuses search');
  }
  await page.evaluate(()=>{location.hash+='&issue=1'});
  await page.waitForFunction(()=>model.detail?.issue.number===1);
  check(true, 'External hash changes still open issue detail');
  check(errors.length===0, 'No navigation runtime errors');
  return {passed:checks.length,checks};
}
