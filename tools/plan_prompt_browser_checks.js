async page => {
  const checks=[],errors=[];
  page.setDefaultTimeout(90000);page.setDefaultNavigationTimeout(120000);
  // Exercise current UI sources even if the fixture binary predates a CSS edit.
  for(const [name,type] of [['app.css','text/css'],['project-settings.js','application/javascript']])
    await page.route(`**/${name}`,route=>route.fulfill({path:`src/issues/web/${name}`,contentType:type}));
  await page.route('**/api/inbox',route=>route.fulfill({json:{ok:true,tasks:[]}}));
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:59693/#project=named%3APlan%20QA');
  await page.reload({waitUntil:'domcontentloaded'});
  await page.waitForFunction(()=>model.project?.id==='named:Plan QA');
  const open=async()=>{
    await page.locator('#project-settings-trigger').click();
    await page.waitForFunction(()=>!document.querySelector('#project-preview-task').disabled&&document.querySelector('#project-instructions-preview').getAttribute('aria-busy')==='false');
  };
  const ready=async text=>page.waitForFunction(text=>document.querySelector('#project-instructions-preview').getAttribute('aria-busy')==='false'&&document.querySelector('#project-instructions-preview').textContent.includes(text),text);
  await open();
  check(await page.locator('#project-settings-form button[type=submit]').isDisabled(),'No initial unsaved changes');
  await page.locator('#project-preview-task').selectOption('plan');await ready('Claim and plan');
  check(await page.locator('#project-settings-form button[type=submit]').isDisabled(),'Task preview is not persisted');
  check(await page.locator('#project-preview-workspace').isDisabled(),'Plan excludes workspace selection');
  check(await page.locator('.workflow-branch.active').count()===1,'Only Plan is included');
  check((await page.locator('#project-instructions-preview').textContent()).includes('hey-boss artifact create'),'Default describes artifact delivery');
  await page.locator('#project-prompt-plan').fill('/goal Claim and plan {{issue_command}}.\n\nCustom scope for {{title}}; save a linked artifact.');
  await ready('Custom scope');
  check(await page.locator('#project-goal-indicator').isVisible(),'Plan prompt enables goal mode');
  check(!(await page.locator('#project-instructions-preview').textContent()).includes('Commit your changes'),'Plan excludes implementation delivery');
  const fail=route=>route.request().postDataJSON()?.operation?.action==='configure_project'?route.fulfill({status:503,json:{ok:false,error:{message:'Synthetic save failure'}}}):route.continue();
  await page.route('**/api/action',fail);
  await page.locator('#project-settings-form button[type=submit]').click();
  await page.locator('#project-settings-error').filter({hasText:'Synthetic save failure'}).waitFor();
  check(await page.locator('#project-prompt-plan').isEnabled(),'Failed save keeps prompt editable');
  check((await page.locator('#project-prompt-plan').inputValue()).includes('Custom scope'),'Failed save retains prompt');
  await page.unroute('**/api/action',fail);
  await page.locator('#project-settings-form button[type=submit]').click();await page.locator('#project-settings-dialog').waitFor({state:'hidden'});
  await open();check((await page.locator('#project-prompt-plan').inputValue()).includes('Custom scope'),'Plan override persists');
  await page.locator('#project-preview-task').selectOption('plan');await ready('Custom scope');
  await page.locator('[data-reset-prompt=plan]').click();await ready('Produce a concrete plan');
  check(await page.locator('#project-source-plan').textContent()==='Built-in default','Reset uses built-in Plan prompt');
  await page.locator('#project-settings-form button[type=submit]').click();await page.locator('#project-settings-dialog').waitFor({state:'hidden'});
  await open();check(await page.locator('#project-prompt-plan').inputValue()==='','Default reset persists');
  await page.locator('#project-settings-cancel').click();
  for(const theme of ['light','dark'])for(const width of [1440,768,390,320]){
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});await page.setViewportSize({width,height:1000});
    await open();await page.locator('#project-preview-task').selectOption('plan');await ready('Claim and plan');
    check(await page.locator('#project-settings-dialog').evaluate(el=>el.scrollWidth<=el.clientWidth+1&&el.getBoundingClientRect().right<=innerWidth),`${theme}/${width} settings fit`);
    await page.locator('#project-prompt-plan').scrollIntoViewIfNeeded();
    if(width<=390)check(await page.locator('#project-prompt-plan').evaluate(el=>parseFloat(getComputedStyle(el).fontSize)>=16),`${theme}/${width} Plan prompt avoids phone focus zoom`);
    await page.screenshot({path:`output/playwright/issue93/${theme}-${width}-settings.png`});
    await page.locator('#project-preview-task').scrollIntoViewIfNeeded();
    await page.screenshot({path:`output/playwright/issue93/${theme}-${width}-preview.png`});
    await page.keyboard.press('Escape');
    check(!await page.locator('#project-settings-dialog').isVisible(),`${theme}/${width} keyboard dismissal`);
    await page.locator('#new-issue').click();
    check(await page.locator('#editor-kind option').allTextContents().then(v=>v.join(',')==='Implement,Plan'),`${theme}/${width} editor offers only Implement and Plan`);
    await page.locator('#editor-kind').selectOption('plan');
    await page.screenshot({path:`output/playwright/issue93/${theme}-${width}-editor.png`});await page.keyboard.press('Escape');
    await page.locator('#quick-issue-open').click();
    check(await page.locator('#quick-issue-kind option').allTextContents().then(v=>v.join(',')==='Implement,Plan'),`${theme}/${width} Quick Add offers only Implement and Plan`);
    await page.locator('#quick-issue-kind').selectOption('plan');
    await page.screenshot({path:`output/playwright/issue93/${theme}-${width}-quick.png`});await page.locator('#quick-issue-close').click();
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${theme}/${width} no horizontal overflow`);
  }
  check(errors.length===0,'No browser errors');
  return {checks:checks.length,errors};
}
