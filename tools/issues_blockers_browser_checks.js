// Run with playwright-cli run-code against the isolated Blockers QA server.
async page => {
  page.setDefaultTimeout(15000);
  const checks=[], errors=[];
  page.on('pageerror',e=>errors.push(e.message));
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  const base='http://127.0.0.1:4796/#project=named%3ABlockers%20QA&state=blocked';
  const action=operation=>page.evaluate(operation=>api(operation,model.project.id),operation);
  const go=async n=>{await page.goto('about:blank');await page.goto(base+`&issue=${n}`);await page.locator('.state-pill').waitFor();};
  await page.keyboard.press('Escape');
  await go(4);
  await action({action:'set_blockers',number:4,blockers:[3],force:false});
  await action({action:'reopen',number:2});
  await action({action:'reopen',number:3});
  await go(4);
  check(await page.locator('.state-pill.blocked').isVisible(),'Linked issue is Blocked');
  check(await page.locator('.blocked-notice button').isDisabled(),'Cannot reopen with unfinished blockers');
  await page.getByRole('button',{name:'Add blocker',exact:true}).click();
  await page.getByLabel('Find a blocking issue').fill('SSO');
  await page.getByRole('button',{name:'#2 Restore GitHub SSO access Open',exact:true}).click();
  await page.locator('#blocker-picker-dialog').waitFor({state:'hidden'});
  await page.getByRole('button',{name:'Remove blocker #2',exact:true}).waitFor();
  check(await page.getByRole('button',{name:'Remove blocker #2',exact:true}).isVisible(),'Picker adds a second linked blocker');
  await action({action:'close',number:2,comment:null,force:false});
  await go(4);
  check(await page.locator('.state-pill.blocked').isVisible(),'Still Blocked until all blockers finish');
  check((await page.locator('.issue-blockers').innerText()).includes('Closed'),'Resolved linked issue remains visible for editing');
  await action({action:'close',number:3,comment:null,force:false});
  await go(4);
  check(await page.locator('.state-pill.open').isVisible(),'Closing last blocker reopens dependent automatically');
  await action({action:'reopen',number:2});
  await go(4);
  check(await page.locator('.state-pill.blocked').isVisible(),'Reopening a blocker blocks dependent again');
  await go(2);
  await page.getByRole('button',{name:'Add blocker',exact:true}).click();
  await page.getByLabel('Find a blocking issue').fill('Audit');
  await page.getByRole('button',{name:'#4 Audit linked blockers Blocked',exact:true}).click();
  await page.getByText('This dependency would create a cycle',{exact:true}).waitFor();
  check(await page.locator('#blocker-picker-dialog').isVisible(),'Cycle rejected without closing picker');
  await page.keyboard.press('Escape');
  await go(4);
  await page.getByRole('button',{name:'Remove blocker #2',exact:true}).click();
  await page.locator('.state-pill.open').waitFor();
  check(await page.getByRole('button',{name:'Remove blocker #2',exact:true}).count()===0,'Removing last active link resumes pickup');
  await action({action:'reopen',number:3});
  await go(4);
  for(const scheme of ['light','dark']) {
    await page.emulateMedia({colorScheme:scheme});
    for(const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:900});
      await go(4);
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`Detail fits ${scheme}/${width}`);
      check(await page.getByRole('button',{name:'Remove blocker #3',exact:true}).isVisible(),`Blocker control visible ${scheme}/${width}`);
      await page.screenshot({path:`output/playwright/blockers/${scheme}-${width}-detail.png`,fullPage:true});
      await page.getByRole('button',{name:'Add blocker',exact:true}).click();
      await page.locator('#blocker-picker-results [data-select-blocker]').first().waitFor();
      check(await page.locator('#blocker-picker-dialog').evaluate(el=>el.scrollWidth<=el.clientWidth),`Picker fits ${scheme}/${width}`);
      await page.screenshot({path:`output/playwright/blockers/${scheme}-${width}-picker.png`});
      await page.keyboard.press('Escape');
      await page.getByRole('button',{name:'All issues',exact:true}).click();
      await page.locator('.issue-blocked-by').first().waitFor();
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`List fits ${scheme}/${width}`);
      check(await page.locator('.issue-row[data-issue-number="4"] .issue-blocked-by').innerText().then(t=>t.includes('#3 Finish native CI')),`List names blocking issue ${scheme}/${width}`);
      await page.screenshot({path:`output/playwright/blockers/${scheme}-${width}-list.png`,fullPage:true});
    }
  }
  await page.emulateMedia({colorScheme:'light'});
  await page.setViewportSize({width:1440,height:900});
  check(errors.length===0,`No browser exceptions: ${errors}`);
  return {checks:checks.length,errors};
}
