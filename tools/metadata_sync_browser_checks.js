// playwright-cli run-code, using metadata_sync_checks.mjs --serve.
async page => {
  const checks=[], errors=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  const onError=error=>errors.push(error.message);
  page.on('pageerror',onError);
  try {
    await page.goto('http://127.0.0.1:59650/');
    await page.waitForFunction(()=>model.csrf && model.projects.length);
    await page.evaluate(async()=>{
      const project=model.projects.find(p=>p.name==='Metadata sync QA').id;
      history.replaceState(null,'',routeHash({...model.route,project,issue:1,view:'issues'}));
      await renderRoute();
    });
    await page.locator('[data-edit]').waitFor();
    const original=await page.evaluate(()=>JSON.stringify(model.detail.issue));
    for(const colorScheme of ['light','dark']) {
      await page.emulateMedia({colorScheme});
      await page.locator('[data-edit]').click();
      await page.locator('#editor-subject').fill('Preserve this unsaved title');
      await page.locator('#editor-body').fill('## Unsaved details\n\nKeep my draft after the denied edit.');
      await page.locator('#editor-labels').fill('new-review-label');
      await page.locator('#editor-labels').press('Enter');
      await page.locator('#editor-submit').click();
      await page.locator('#editor-error').waitFor({state:'visible'});
      check(await page.locator('#editor-error').evaluate(el=>el.getBoundingClientRect().bottom<=document.querySelector('#editor-submit').getBoundingClientRect().top),'Rejection is visible above the footer in '+colorScheme);
      check((await page.locator('#editor-error').innerText()).includes('Changes were not saved'),'Explicit edit rejection in '+colorScheme);
      check(await page.locator('#editor-conflict').isHidden(),'No overwrite option for allocation denial in '+colorScheme);
      check(await page.locator('#editor-subject').inputValue()==='Preserve this unsaved title','Title draft retained in '+colorScheme);
      check((await page.locator('#editor-body').inputValue()).includes('Keep my draft'),'Body draft retained in '+colorScheme);
      check(await page.evaluate(()=>pendingMutation.size===0),'Denied edit is not queued for replay in '+colorScheme);
      for(const width of [1440,768,390,320]) {
        await page.setViewportSize({width,height:900});
        await page.locator('#editor-error').scrollIntoViewIfNeeded();
        check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'No page overflow '+colorScheme+' '+width);
        check(await page.locator('#editor-error').evaluate(el=>el.scrollWidth<=el.clientWidth),'Error wraps '+colorScheme+' '+width);
        check(await page.locator('#editor-submit').evaluate(el=>{const r=el.getBoundingClientRect();return r.left>=0&&r.right<=innerWidth&&r.top>=0&&r.bottom<=innerHeight;}),'Save remains reachable '+colorScheme+' '+width);
        if([1440,320].includes(width))await page.screenshot({path:`output/playwright/issue150/${colorScheme}-${width}.png`});
      }
      await page.locator('#editor-cancel').click();
      check(await page.locator('[data-edit]').evaluate(el=>el===document.activeElement),'Cancel restores keyboard focus in '+colorScheme);
      check(await page.evaluate(()=>JSON.stringify(model.detail.issue))===original,'Denied edit leaves canonical issue unchanged in '+colorScheme);
      const reopen=page.locator('[data-action="reopen"]');
      await reopen.click();
      await page.waitForFunction(()=>document.querySelector('#toast')?.textContent.includes('Changes were not saved'));
      check(await page.evaluate(()=>model.detail.issue.state==='closed'),'Denied reopen keeps closed state in '+colorScheme);
      check(await reopen.isEnabled(),'Reopen recovers after denial in '+colorScheme);
    }
    check(errors.length===0,'No browser runtime errors');
    if(checks.length!==45)throw Error('Incomplete browser graph '+checks.length+'/45');
    return {completed:checks.length,expected:45,checks};
  }finally{page.off('pageerror',onError);}
}
