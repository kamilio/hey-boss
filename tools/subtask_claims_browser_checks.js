// Run through playwright-cli run-code against an isolated native or paired fixture.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  const onError = error => errors.push(error.message);
  page.on('pageerror', onError);
  try {
    await page.reload({waitUntil:'domcontentloaded'});
    await page.waitForFunction(() => model.csrf && model.project);
    const project = await page.evaluate(async () => {
      const p = 'Claim safety QA ' + crypto.randomUUID();
      const parent = await api({action:'create',title:'Active parent',body:'Parent work stays assigned.',labels:[]},p);
      await api({action:'create',title:'Completed child',body:'Done.',labels:[]},p);
      await api({action:'close',number:2,force:false},p);
      await api({action:'add_subtask',number:1,child:2},p);
      await api({action:'create',title:'Open follow-up',body:'Organize without takeover.',labels:[]},p);
      await api({action:'assign_boss',number:1,force:false},p);
      return parent.project.id;
    });
    await page.evaluate(async project => {
      history.pushState(null,'',routeHash({...model.route,project,issue:1,view:'issues'}));
      await renderRoute();
    },project);
    await page.waitForFunction(() => model.detail?.issue.title === 'Active parent');
    const before = await page.evaluate(() => JSON.stringify(model.detail.issue));
    await page.locator('[data-create-subtask]').click();
    check(await page.locator('#editor-subtask-help').isVisible(),'Creation explains scheduling before submission');
    check((await page.locator('#editor-subtask-help').innerText()).includes('mindmap'),'Creation offers mindmap grouping');
    await page.locator('#editor-subject').fill('Keep this follow-up draft');
    await page.locator('#editor-body').fill('## Unfinished work\n\nPreserve this text.');
    await page.locator('#editor-submit').click();
    await page.locator('#editor-error').waitFor({state:'visible'});
    check((await page.locator('#editor-error').innerText()).includes('claim'),'Rejected creation explains claim protection');
    check(await page.locator('#editor-conflict').isHidden(),'Claim rejection does not offer a version replacement');
    check(await page.evaluate(() => pendingMutation.size === 0),'Rejected creation is not queued for automatic replay');
    check(await page.locator('#editor-subject').inputValue() === 'Keep this follow-up draft','Rejected creation preserves the title');
    check((await page.locator('#editor-body').inputValue()).includes('Preserve this text.'),'Rejected creation preserves Markdown');
    for (const width of [320,390,768,1440]) {
      await page.setViewportSize({width,height:900});
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),'Creation has no horizontal overflow at '+width);
      check(await page.locator('#editor-error').evaluate(el => el.scrollWidth <= el.clientWidth),'Claim error wraps at '+width);
    }
    await page.locator('#editor-cancel').click();
    check(await page.locator('[data-create-subtask]').evaluate(el => el === document.activeElement),'Cancel returns focus to Add subtask');
    await page.locator('[data-add-existing-subtask]').click();
    await page.locator('[data-existing-subtask="3"]').waitFor();
    check((await page.locator('#subtask-picker-help').innerText()).includes('Blocked'),'Existing issue picker explains scheduling');
    await page.locator('[data-existing-subtask="3"]').click();
    await page.locator('#subtask-picker-error').waitFor({state:'visible'});
    check((await page.locator('#subtask-picker-error').innerText()).includes('mindmap'),'Link rejection offers safe grouping');
    check(await page.locator('#subtask-picker-dialog').isVisible(),'Rejected link stays in the picker');
    check(await page.evaluate(() => pendingMutation.size === 0),'Rejected link is not queued for automatic replay');
    for (const width of [320,390,768,1440]) {
      await page.setViewportSize({width,height:900});
      check(await page.locator('#subtask-picker-dialog').evaluate(el => {const r=el.getBoundingClientRect();return r.left>=0&&r.right<=innerWidth&&r.top>=0&&r.bottom<=innerHeight;}),'Picker fits '+width);
    }
    await page.keyboard.press('Escape');
    check(await page.locator('[data-add-existing-subtask]').evaluate(el => el === document.activeElement),'Escape returns picker focus');
    const state = await page.evaluate(async project => ({parent:(await api({action:'view',number:1},project)).issue,issues:(await api({action:'list',state:'all',mine:false,unassigned:false,labels:[],limit:100,offset:0,all:true},project)).issues}),project);
    check(JSON.stringify(state.parent) === before,'Rejected creation and link preserve the entire parent');
    check(state.issues.length === 3,'Rejected creation leaves no child issue');
    check(state.issues.find(i=>i.number===3).parent === null,'Rejected link leaves follow-up unlinked');
    await page.evaluate(() => navigate({issue:null}));
    await page.locator('#new-issue').click();
    check(await page.locator('#editor-subtask-help').isHidden(),'Ordinary issue creation has no subtask guidance');
    await page.locator('#editor-cancel').click();
    check(errors.length===0,'No browser runtime errors');
    if (checks.length !== 30) throw Error('Incomplete browser check graph: '+checks.length+'/30');
    return {completed:checks.length,expected:30,checks,project};
  } finally { page.off('pageerror',onError); }
}
