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
 await page.locator('#job-frequency').selectOption('hours');await page.locator('#job-interval').fill('3');await page.locator('#job-minute').fill('25');await save('25 */3 * * *');
 check(await page.locator('#job-frequency').inputValue()==='hours','Hour interval restores as a preset');
 check(await page.locator('#job-interval').inputValue()==='3'&&await page.locator('#job-minute').inputValue()==='25','Interval and starting minute restore');
 check((await page.locator('#job-preview strong').textContent())==='Every 3 hours at :25','Readable hourly cadence');
 const occurrences=await page.locator('#job-preview li').allTextContents();
 check(occurrences.every(value=>value.includes(':25')),'Real backend preview uses the minute offset');
 for(const invalid of ['', '0', '24', '2.5']){await page.locator('#job-interval').fill(invalid);check(await page.locator('#job-form').evaluate(form=>!form.checkValidity()),'Invalid interval cannot save: '+invalid);check(await page.locator('#job-cron').inputValue()==='','Invalid interval clears stale cron');}
 await page.locator('#job-interval').fill('5');await save('25 */5 * * *');
 check((await page.locator('#job-preview strong').textContent()).includes('restarting each day'),'Uneven daily step is explicit');
 await page.locator('#job-interval').fill('3');await save('25 */3 * * *');
 for(const width of [1440,768,390,320])for(const theme of ['light','dark']){
  await page.setViewportSize({width,height:800});await page.emulateMedia({colorScheme:theme});
  await page.locator('.job-schedule-editor').evaluate(el=>el.scrollIntoView({block:'start'}));
  check(await page.locator('#job-panel').evaluate(el=>el.scrollWidth<=el.clientWidth),`${width} ${theme}: schedule fits modal`);
  for(const selector of ['#job-close','#job-save','#job-minute','#job-interval']){const box=await page.locator(selector).boundingBox();check(box.x>=0&&box.x+box.width<=width&&box.y>=0&&box.y+box.height<=800,`${width} ${theme}: ${selector} is visible`);}
  const box=await page.locator('#job-dialog').boundingBox();check(Math.abs(box.x-(width-box.width)/2)<2,'Job dialog is centered');
  await page.screenshot({path:`${outputDir}/${paired?'paired':'desktop'}-hours-${width}-${theme}.png`});
 }
 await page.setViewportSize({width:1440,height:1000});await page.emulateMedia({colorScheme:'light'});
 await page.locator('#job-save').focus();await page.keyboard.press('Tab');check(await page.locator('#job-reload').evaluate(el=>el===document.activeElement),'Footer actions follow keyboard order');
 await page.locator('#job-new').evaluate(el=>el.focus());check(await page.locator('#job-dialog').evaluate(el=>el.contains(document.activeElement)),'Modal prevents focus moving to the background');
 await page.keyboard.press('Escape');await page.waitForFunction(()=>!document.querySelector('#job-dialog').open);
 await page.locator('.job-card[href$="job='+id+'"]').click();await preview();check(await page.locator('#job-cron').inputValue()==='25 */3 * * *','Reopen restores hourly cadence');
 await page.locator('#job-minute').fill('26');
 page.removeAllListeners('dialog');page.once('dialog',dialog=>dialog.dismiss());await page.keyboard.press('Escape');
 check(await page.locator('#job-dialog').evaluate(el=>el.open),'Cancelled discard keeps the modal open');
 check(await page.locator('#job-minute').inputValue()==='26','Cancelled discard keeps the draft');
 page.on('dialog',dialog=>dialog.accept());await page.keyboard.press('Escape');await page.waitForFunction(()=>!document.querySelector('#job-dialog').open);
 await page.locator('.job-card[href$="job='+id+'"]').click();await preview();check(await page.locator('#job-minute').inputValue()==='25','Discard does not save draft');
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
 await page.locator('.job-card[href$="job='+id+'"]').waitFor({state:'visible'});
 check((await page.locator('.job-cron-caption').allTextContents()).every(value=>value==='30 9 * * 1,3,5'),'List searches the raw cron expression');check((await page.locator('.job-row').filter({has:page.locator('.job-card[href$="job='+id+'"]')}).locator('.job-cron-caption').textContent())==='30 9 * * 1,3,5','List displays the saved cron');
 await page.screenshot({path:`${outputDir}/${paired?'paired':'desktop'}-cron-list.png`});
 await page.reload();await page.locator('.job-card[href$="job='+id+'"]').click();await preview();check(await page.locator('#job-cron').inputValue()==='30 9 * * 1,3,5','Reload retains persisted schedule');
 await page.locator('#job-delete').click();await page.waitForFunction(()=>document.querySelector('.job-panel-subtitle')?.textContent.includes('Deleted'));
 check(await page.locator('#job-cron').isDisabled(),'Deleted cron is read-only');check(errors.length===0,'No JavaScript errors');return {surface:paired?'paired':'desktop',checks:checks.length};
}
