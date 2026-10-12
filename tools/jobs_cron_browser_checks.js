// Run against serve_jobs_ui_fixture.mjs; all mutations stay in its disposable store.
async (page, outputDir='/tmp/hb-jobs-cron-qa') => {
 const origin=new URL(page.url()).origin,paired=origin.endsWith('52049'),project='named:Jobs UI QA';
 if(!['http://127.0.0.1:59649','http://127.0.0.1:52049'].includes(origin))throw Error('Isolated fixture required');
 const checks=[],errors=[];
 const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 page.on('pageerror',error=>errors.push(error.message));page.on('dialog',dialog=>dialog.accept());page.setDefaultTimeout(15000);
 if(paired){const {code}=await(await page.request.get(origin+'/fixture-pairing')).json();await page.request.post(origin+'/api/pair',{data:{code}});}
 await page.route('**/api/jobs/runtime',route=>route.fulfill({json:{ok:true,machine:'fixture',machines:[],catalog:{models:[]}}}));
 const markdown=await(await page.request.get('http://127.0.0.1:52049/fixture-instructions.md')).text();
 await page.goto(origin+'/jobs#project='+encodeURIComponent(project));
 await page.locator('#job-new').click();await page.locator('#job-name').fill('Cron controls');await page.locator('#job-markdown').fill(markdown);await page.locator('#job-model').fill('fixture-model');await page.locator('#job-enabled').uncheck();await page.locator('#job-timezone').fill('America/Chicago');
 check(await page.locator('#job-cron').isVisible(),'Cron is visible for the default daily schedule');
 const preview=async()=>{await page.waitForFunction(()=>document.querySelectorAll('#job-preview li').length===3);};
 const save=async expected=>{
  check(await page.locator('#job-cron').inputValue()===expected,'Controls produce '+expected);await preview();
  const revision=await page.locator('.job-panel-subtitle').textContent();
  await page.locator('#job-save').click();await page.waitForFunction(previous=>document.querySelector('.job-panel-subtitle')?.textContent!==previous,revision);
  check(await page.locator('#job-cron').inputValue()===expected,'Saved cron round trip: '+expected);
  check(await page.locator('#job-markdown').inputValue()===markdown,'Schedule edits preserve exact instructions');await preview();
 };
 await page.locator('#job-frequency').selectOption('minutes');await page.locator('#job-interval').fill('15');await save('*/15 * * * *');
 const id=new URLSearchParams(new URL(page.url()).hash.slice(1)).get('job');
 await page.locator('#job-frequency').selectOption('hourly');await page.locator('#job-minute').fill('45');await save('45 * * * *');
 check(await page.locator('#job-minute').inputValue()==='45','Hourly offset restores');
 await page.locator('#job-minute').fill('');check(await page.locator('#job-cron').inputValue()==='','Empty hourly minute does not silently select zero');
 await page.locator('#job-minute').fill('60');check(await page.locator('#job-form').evaluate(form=>!form.checkValidity()),'Out-of-range minutes cannot save');await page.locator('#job-minute').fill('45');
 await page.locator('#job-frequency').selectOption('weekly');await page.locator('#job-time').fill('17:35');
 await page.getByRole('button',{name:'Wednesday',exact:true}).click();await page.getByRole('button',{name:'Friday',exact:true}).focus();await page.keyboard.press('Space');await save('35 17 * * 1,3,5');
 check(await page.getByRole('button',{name:'Friday',exact:true}).getAttribute('aria-pressed')==='true','Keyboard day toggle restores after save');
 for(const name of ['Monday','Wednesday','Friday'])await page.getByRole('button',{name,exact:true}).click();
 check(await page.locator('#job-cron').inputValue()==='','No selected days clears the previous schedule');
 check(await page.locator('#job-form').evaluate(form=>!form.checkValidity()),'Empty day selection cannot save');
 await page.getByRole('button',{name:'Sunday',exact:true}).click();await save('35 17 * * 0');
 await page.locator('#job-frequency').selectOption('monthly');await page.locator('#job-monthday').fill('31');await save('35 17 31 * *');
 check((await page.locator('#job-monthday-help').textContent()).includes('skipped'),'Short-month behavior explained');
 check((await page.locator('#job-preview').textContent()).includes('31'),'Actual monthly preview contains the selected date');
 await page.locator('#job-cron').fill('0 9,17 * * MON-FRI');await page.locator('#job-timezone').focus();await save('0 9,17 * * MON-FRI');
 check(await page.locator('#job-frequency').inputValue()==='custom','Complex custom expression stays custom');
 await page.locator('#job-cron').fill('0 9 31 FEB *');await page.waitForFunction(()=>document.querySelector('#job-preview').textContent.includes('never matches'));
 check(await page.locator('#job-cron').getAttribute('aria-invalid')==='true','Impossible date is marked invalid');
 await page.locator('#job-cron').fill('*/7 9-17 * JAN,MAR MON-FRI');await save('*/7 9-17 * JAN,MAR MON-FRI');
 await page.locator('.job-cron-reference summary').click();await page.locator('[data-cron-example="0 9 1 JAN,APR,JUL,OCT *"]').click();await save('0 9 1 JAN,APR,JUL,OCT *');
 check(await page.locator('#job-cron').getAttribute('aria-invalid')===null,'Valid input clears error state');
 await page.locator('#job-timezone').fill('Imaginary/Zone');await page.waitForFunction(()=>document.querySelector('#job-timezone').getAttribute('aria-invalid')==='true');
 check(await page.locator('#job-cron').getAttribute('aria-invalid')===null,'Timezone errors identify the timezone field');await page.locator('#job-timezone').fill('America/Chicago');await preview();
 await page.locator('#job-frequency').selectOption('weekly');await page.locator('#job-time').fill('09:30');await page.getByRole('button',{name:'Wednesday',exact:true}).click();await page.getByRole('button',{name:'Friday',exact:true}).click();await save('30 9 * * 1,3,5');
 for(const width of [1440,768,390,320])for(const theme of ['light','dark']){
  await page.setViewportSize({width,height:1000});await page.emulateMedia({colorScheme:theme});
  await page.locator('.job-schedule-editor').evaluate(el=>el.scrollIntoView({block:'start'}));
  check(await page.locator('#job-dialog').evaluate(el=>el.scrollWidth<=el.clientWidth),`${width} ${theme}: editor fits`);
  for(const selector of ['#job-cron','.job-weekdays','.job-cron-legend','#job-preview']){const box=await page.locator(selector).boundingBox();check(box.x>=0&&box.x+box.width<=width,`${width} ${theme}: ${selector} fits`);}
  await page.screenshot({path:`${outputDir}/${paired?'paired':'desktop'}-cron-${width}-${theme}.png`});
 }
 await page.setViewportSize({width:1440,height:1000});await page.emulateMedia({colorScheme:'light'});
 await page.locator('.job-cron-reference summary').click();await page.locator('.job-schedule-editor').evaluate(el=>el.scrollIntoView({block:'start'}));await page.screenshot({path:`${outputDir}/${paired?'paired':'desktop'}-cron-guide.png`});
 await page.locator('#job-close').click();await page.waitForFunction(()=>!document.querySelector('#job-dialog').open);await page.locator('#jobs-search').fill('30 9 * * 1,3,5');
 check(await page.locator('.job-row').count()===1,'List searches the raw cron expression');check((await page.locator('.job-cron-caption').textContent())==='30 9 * * 1,3,5','List displays the saved cron');
 await page.screenshot({path:`${outputDir}/${paired?'paired':'desktop'}-cron-list.png`});
 await page.reload();await page.locator('.job-card[href$="job='+id+'"]').click();await preview();check(await page.locator('#job-cron').inputValue()==='30 9 * * 1,3,5','Reload retains persisted schedule');
 await page.locator('#job-delete').click();await page.waitForFunction(()=>document.querySelector('.job-panel-subtitle')?.textContent.includes('Deleted'));
 check(await page.locator('#job-cron').isDisabled(),'Deleted cron is read-only');check(errors.length===0,'No JavaScript errors');return {surface:paired?'paired':'desktop',checks:checks.length};
}
