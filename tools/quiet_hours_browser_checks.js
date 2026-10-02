// playwright-cli run-code --filename tools/quiet_hours_browser_checks.js
async page => {
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const check=(value,message)=>{if(!value)throw Error(message);};
 const open=async()=>{await page.locator('#self-avatar').click();await page.locator('#global-settings-trigger').click();await page.waitForFunction(()=>!document.querySelector('#global-boss-name').disabled);};
 await page.goto('http://127.0.0.1:59651');await open();
 check(await page.locator('#global-quiet-enabled').isChecked(),'Enabled by default');
 check(await page.locator('#global-quiet-start').inputValue()==='22:00','Default start');
 check(await page.locator('#global-quiet-end').inputValue()==='07:00','Default end');
 check(await page.locator('#global-settings-submit').isDisabled(),'Unchanged form');
 for(const [name,width,height,theme] of [['desktop',1280,960,'light'],['phone',390,844,'light'],['narrow',320,568,'light'],['dark',390,844,'dark']]){
  await page.setViewportSize({width,height});await page.emulateMedia({colorScheme:theme});
  check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Page fits '+name);
  check(await page.locator('#global-settings-dialog').evaluate(e=>e.scrollWidth<=e.clientWidth+1),'Dialog fits '+name);
  await page.locator('#global-quiet-zone').scrollIntoViewIfNeeded();
  await page.screenshot({path:'/tmp/hb-quiet-hours/'+name+'.png'});
 }
 await page.locator('#global-quiet-start').fill('21:30');await page.locator('#global-quiet-end').fill('21:30');
 await page.locator('#global-settings-submit').click();check(await page.evaluate(()=>saved.length===0),'Reject equal times');
 await page.locator('#global-quiet-end').fill('08:15');await page.locator('#global-quiet-zone').selectOption('Europe/Warsaw');
 await page.locator('#global-settings-submit').click();check(await page.evaluate(()=>saved.at(-1).quiet_hours.time_zone==='Europe/Warsaw'),'Save schedule');
 await open();check(await page.locator('#global-quiet-start').inputValue()==='21:30','Reopen saved times');
 await page.locator('#global-quiet-enabled').uncheck();check(await page.locator('#global-quiet-start').isDisabled(),'Disable time fields');
 await page.locator('#global-settings-submit').click();await open();check(!await page.locator('#global-quiet-enabled').isChecked(),'Off persists');
 await page.locator('#global-quiet-enabled').check();await page.locator('#global-quiet-start').fill('20:00');
 await page.evaluate(()=>failure='Settings changed elsewhere');await page.locator('#global-settings-submit').click();
 check(await page.locator('#global-settings-error').isVisible(),'Save error visible');check(await page.locator('#global-quiet-start').inputValue()==='20:00','Draft retained');
 await page.locator('#global-settings-reload').click();check(await page.locator('#global-quiet-start').inputValue()==='20:00','Conflict reload retains draft');
 await page.evaluate(()=>failure=null);await page.locator('#global-settings-submit').click();
 await open();await page.keyboard.press('Escape');check(await page.locator('#self-avatar').evaluate(e=>e===document.activeElement),'Escape restores focus');
 await page.goto('http://127.0.0.1:59651/?phone=1#settings=1');await page.locator('#global-settings-dialog').waitFor({state:'visible'});check(await page.locator('#global-settings-dialog').isVisible(),'Phone settings link opens dialog');
 check(errors.length===0,errors.join('\n'));return 'PASS desktop, phone, narrow, dark, validation, persistence, enable/disable, conflict recovery, keyboard and deep link';
}
