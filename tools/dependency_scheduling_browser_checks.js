// playwright-cli run-code --filename, against an isolated native or paired issue UI.
async page => {
  const checks = [], stages = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  const onError = error => errors.push(error.message);
  page.on('pageerror', onError);
  page.setDefaultTimeout(20000);
  const base = await page.evaluate(() => location.origin);
  const surface = await page.evaluate(() => location.pathname === '/issues' ? 'paired' : 'native');
  try {
    await page.reload({waitUntil:'domcontentloaded'});
    await page.waitForFunction(() => model.csrf && model.project);
    const project = await page.evaluate(async () => {
      const p = 'Dependency scheduling ' + crypto.randomUUID();
      const root = await api({action:'create',title:'Connector integration',body:'Independent branches share a parent and explicit prerequisites.',labels:[]},p);
      for (const title of ['Define the connector contract','Build Settings independently','Implement OAuth after the contract'])
        await api({action:'create_subtask',number:1,title,body:'Keep the parent relationship and declared dependencies.',labels:[]},p);
      await api({action:'set_blockers',number:4,blockers:[2],force:false},p);
      await api({action:'block',number:4,comment:null,force:false},p);
      return root.project.id;
    });
    const view = async number => {
      await page.evaluate(async ({project,number}) => {
        history.pushState(null,'',routeHash({...model.route,project,issue:number,view:'issues'}));
        await renderRoute();
      },{project,number});
      await page.waitForFunction(number => model.detail?.issue.number === number,number);
    };
    const settings = async () => {
      await page.locator('#project-settings-trigger').click();
      await page.locator('#project-subtask-scheduling').waitFor();
      await page.waitForFunction(() => !!projectSettingsOriginal && !document.querySelector('#project-subtask-scheduling').disabled);
    };
    await view(4);
    check(await page.getByRole('heading',{name:'Manual hold',exact:true}).isVisible(),'Manual hold is distinguished from dependencies');
    check(await page.locator('.issue-blockers').innerText().then(t=>t.includes('#2') && t.includes('#3')),'Sequential mode shows effective earlier siblings');
    await page.locator('.blocked-notice [data-action="clear_manual_hold"]').click();
    await page.getByRole('heading',{name:'Waiting for dependencies',exact:true}).waitFor();
    check(await page.evaluate(() => model.detail.issue.state === 'blocked' && !model.detail.issue.manual_blocked),'Clearing hold keeps automatic blocking');
    check(await page.locator('[data-action="clear_manual_hold"]').count()===0,'Resolved hold cannot be cleared again in the UI');
    stages.push('manual hold');

    await settings();
    check(await page.locator('#project-subtask-scheduling').inputValue()==='sequential','Existing projects default to sequential');
    await page.locator('#project-subtask-scheduling').selectOption('explicit');
    check((await page.locator('#project-scheduling-help').innerText()).includes('Add blocker'),'Explicit mode explains intentional sequences');
    for (const theme of ['light','dark']) {
      await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
      for (const width of [1440,768,390,320]) {
        await page.setViewportSize({width,height:900});
        await page.locator('#project-subtask-scheduling').scrollIntoViewIfNeeded();
        check(await page.locator('#project-settings-dialog').evaluate(el=>{const r=el.getBoundingClientRect();return r.left>=0&&r.right<=innerWidth&&r.top>=0&&r.bottom<=innerHeight;}),`Settings dialog fits ${theme}/${width}`);
        check(await page.locator('#project-subtask-scheduling').evaluate(el=>{const r=el.getBoundingClientRect();return r.width>100&&r.left>=0&&r.right<=innerWidth;}),`Scheduling control fits ${theme}/${width}`);
        check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`No horizontal overflow ${theme}/${width}`);
        await page.screenshot({path:`output/playwright/issue149/${surface}-settings-${theme}-${width}.png`});
      }
    }
    await page.keyboard.press('Escape');
    check(await page.locator('#project-settings-trigger').evaluate(el=>el===document.activeElement),'Escape restores settings trigger focus');
    await settings();
    check(await page.locator('#project-subtask-scheduling').inputValue()==='sequential','Cancel discards unsaved mode');
    await page.locator('#project-subtask-scheduling').selectOption('explicit');
    await page.locator('#project-settings-form button[type=submit]').click();
    await page.locator('#project-settings-dialog').waitFor({state:'hidden'});
    await page.waitForFunction(()=>model.detail?.issue.blocked_by.length===1);
    check(await page.evaluate(()=>model.detail.issue.blocked_by[0].number===2 && model.detail.issue.blocked_by[0].source==='linked'),'Saved explicit mode retains only declared blocker');
    stages.push('settings persistence and responsive design');

    await view(3);
    check(await page.evaluate(()=>model.detail.issue.state==='open' && model.detail.issue.parent.number===1),'Independent sibling becomes open without losing its parent');
    await view(1);
    check((await page.locator('.subtasks-card').innerText()).includes('Explicit dependencies'),'Parent displays scheduling mode');
    check(await page.evaluate(()=>model.detail.issue.state==='blocked'),'Parent still waits for unfinished subtasks');
    await page.screenshot({path:`output/playwright/issue149/${surface}-parent-mobile.png`});
    stages.push('grouping and independent work');

    await page.evaluate(async project=>{
      await api({action:'configure_project',prs_enabled:true},project);
      await api({action:'add_pull_request',number:2,url:'https://github.com/example/connectors/pull/2',purpose:'fix'},project);
      await api({action:'ready',number:2,force:false},project);
    },project);
    await view(4);
    check(await page.evaluate(()=>model.detail.issue.state==='open' && model.detail.issue.dependency_context[0].state==='ready'),'Ready handoff unblocks declared dependent');
    await page.evaluate(async project=>{
      await api({action:'assign_boss',number:4,force:false},project);
    },project);
    stages.push('Ready handoff');

    await page.setViewportSize({width:390,height:900});
    await settings();
    await page.locator('#project-subtask-scheduling').selectOption('sequential');
    await page.locator('#project-settings-form button[type=submit]').click();
    await page.locator('#project-settings-error').waitFor();
    check((await page.locator('#project-settings-error').innerText()).includes('#4'),'Claim conflict identifies affected issue');
    check(await page.locator('#project-settings-error').evaluate(el=>el.scrollWidth<=el.clientWidth),'Claim conflict wraps on phone');
    await page.locator('#project-settings-error').scrollIntoViewIfNeeded();
    await page.screenshot({path:`output/playwright/issue149/${surface}-claim-conflict-mobile.png`});
    await page.locator('#project-settings-cancel').click();
    const kept = await page.evaluate(async project=>({settings:await api({action:'project_settings'},project),issue:(await api({action:'view',number:4},project)).issue}),project);
    check(kept.settings.subtask_scheduling==='explicit' && kept.issue.assignee==='human:boss','Rejected mode change preserves setting and claim');
    stages.push('claim protection');

    await page.evaluate(async project=>{
      await api({action:'assign_boss',number:3,force:false},project);
      await api({action:'reopen',number:2},project);
    },project);
    await view(3);
    check(await page.evaluate(()=>model.detail.issue.blocked_by.length===0 && model.detail.issue.assignee==='human:boss'),'Independent running sibling retains its claim without blockers');
    check(await page.evaluate(()=>!model.detail.comments.some(c=>c.body.startsWith('Dependency rework:'))),'Independent sibling receives no rework notice');
    await view(4);
    check(await page.evaluate(()=>model.detail.comments.filter(c=>c.body.startsWith('Dependency rework: upstream tasks [2]')).length===1),'Declared dependency still produces one rework notice');
    check(await page.evaluate(()=>model.detail.issue.assignee==='human:boss'),'Real rework notice preserves the running claim');
    for (const theme of ['light','dark']) {
      await page.emulateMedia({colorScheme:theme});
      for (const width of [1440,390,320]) {
        await page.setViewportSize({width,height:900});
        await page.getByText('Dependency rework: upstream tasks [2]',{exact:false}).first().scrollIntoViewIfNeeded();
        check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`Rework notice wraps ${theme}/${width}`);
        await page.screenshot({path:`output/playwright/issue149/${surface}-rework-${theme}-${width}.png`});
      }
    }
    stages.push('dependency notice and claim preservation');

    await page.evaluate(async project=>{
      await api({action:'unassign',number:4,force:false},project);
      await api({action:'block',number:4,comment:null,force:false},project);
    },project);
    await view(4);
    for (const theme of ['light','dark']) {
      await page.emulateMedia({colorScheme:theme});
      for (const width of [1440,320]) {
        await page.setViewportSize({width,height:900});
        check(await page.locator('.blocked-notice').evaluate(el=>el.scrollWidth<=el.clientWidth),`Manual hold banner fits ${theme}/${width}`);
        await page.screenshot({path:`output/playwright/issue149/${surface}-hold-${theme}-${width}.png`});
      }
    }
    check(errors.length===0,'No browser runtime errors');
    stages.push('manual hold visual states');
    if(stages.length!==7) throw Error('Incomplete browser graph');
    return {completed:stages.length,expected:7,stages,checks:checks.length,project,surface,base};
  } finally {page.off('pageerror',onError);}
}
