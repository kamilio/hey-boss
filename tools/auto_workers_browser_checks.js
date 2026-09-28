// Run with playwright-cli run-code --filename against serve_auto_workers_fixture.mjs.
async page => {
  const checks=[],errors=[];
  const check=(value,name)=>{if(!value)throw Error(name);checks.push(name);};
  page.on('pageerror',error=>errors.push(error.message));
  await page.setViewportSize({width:1440,height:1050});
  await page.goto('http://127.0.0.1:59688/workers');
  await page.locator('.device-group').first().waitFor();
  check(await page.locator('.device-group').count()===3,'All machines remain visible');
  check(await page.locator('.device-row').count()===4,'Independent workers stay outside managed fleet');
  check((await page.locator('[data-worker="paused"] .worker-state').innerText())==='Paused','Paused worker status is explicit');
  check((await page.locator('[data-host="devbox"] .device-summary').allTextContents()).includes('Saved · waiting for connection'),'Offline changes show pending application');
  check(await page.locator('.worker-task').count()===1,'Active tasks link to their conversations');
  check((await page.locator('.fleet-stat strong').allTextContents()).join(',')==='2 / 3,4,1,7','Capacity and active counts reflect managed workers');
  await page.screenshot({path:'output/playwright/auto-workers/overview-light.png',fullPage:true});
  await page.emulateMedia({colorScheme:'dark'});
  await page.screenshot({path:'output/playwright/auto-workers/overview-dark.png',fullPage:true});
  await page.emulateMedia({colorScheme:'light'});
  await page.locator('#config-editor>summary').click();
  await page.waitForFunction(()=>!document.getElementById('config-text').disabled);
  const original=await page.locator('#config-text').inputValue();
  check(await page.locator('#config-save').isDisabled(),'Unchanged file cannot be resaved accidentally');
  await page.locator('#config-text').fill(original.replace('concurrency: 2','concurrency: 0'));
  await page.locator('#config-validate').click();
  await page.waitForFunction(()=>document.getElementById('config-status').textContent.includes('between 1 and 1024'));
  check(await page.locator('#config-save').isDisabled(),'Invalid YAML settings block saving');
  const edited=original.replace('concurrency: 2','concurrency: 3');
  await page.locator('#config-text').fill(edited);
  await page.locator('#config-validate').click();
  await page.waitForFunction(()=>!document.getElementById('config-save').disabled);
  check((await page.locator('#config-changes').innerText()).includes('Update poe-code on local'),'Preview names affected worker and machine');
  await page.locator('#config-save').click();
  await page.waitForFunction(()=>document.getElementById('config-status').textContent.startsWith('Saved.'));
  check(await page.locator('#config-text').inputValue()===edited,'Saved text remains exact');
  await page.locator('#config-text').fill(edited+'# unsaved note\n');
  await page.evaluate(()=>fetch('/fixture/change'));
  await page.locator('#config-validate').click();
  await page.waitForFunction(()=>/changed/.test(document.getElementById('config-status').textContent));
  check((await page.locator('#config-text').inputValue()).endsWith('# unsaved note\n'),'Conflicting changes retain the user draft');
  check(await page.locator('#config-save').isDisabled(),'Stale revision cannot be saved');
  await page.screenshot({path:'output/playwright/auto-workers/editor-desktop.png',fullPage:true});
  for(const width of [390,320]){
    await page.setViewportSize({width,height:844});
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),width+'px layout has no horizontal overflow');
  }
  await page.screenshot({path:'output/playwright/auto-workers/editor-mobile-light.png',fullPage:true});
  await page.emulateMedia({colorScheme:'dark'});
  await page.screenshot({path:'output/playwright/auto-workers/editor-mobile-dark.png',fullPage:true});
  await page.locator('#config-text').fill(edited);
  check(errors.length===0,'No browser JavaScript errors');
  return {passed:checks.length,checks};
}
