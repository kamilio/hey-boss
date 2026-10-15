// Run against serve_jobs_ui_fixture.mjs; mutations stay in its disposable store.
async (page) => {
 const origin=page.url().split('/').slice(0,3).join('/');
 if(origin!=='http://127.0.0.1:59649')throw Error('Isolated native fixture required');
 const check=(ok,name)=>{if(!ok)throw Error(name);};
 page.setDefaultTimeout(15000);
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.route('**/api/jobs/runtime',r=>r.fulfill({json:{ok:true,machine:'fixture',catalog:{models:[]},machines:[{node:'primary',host:'Mac Studio',state:'connected'},{node:'fallback',host:'Build server',state:'disconnected'}]}}));
 await page.goto(origin+'/jobs#project='+encodeURIComponent('named:Jobs UI QA'));
 await page.locator('#job-new').click();
 check(await page.locator('#job-placement').inputValue()==='auto','Defaults to automatic');
 check(await page.locator('#job-targets').isHidden(),'Automatic hides targets');
 await page.locator('#job-name').fill('Machine priority');
 await page.locator('#job-markdown').fill(await(await page.request.get('http://127.0.0.1:52049/fixture-instructions.md')).text());
 await page.locator('#job-model').fill('fixture-model');await page.locator('#job-enabled').uncheck();
 await page.locator('#job-placement').selectOption('specific');
 await page.locator('#job-machine-add').click();await page.locator('#job-machine-add').click();
 check(await page.locator('[data-machine="0"]').inputValue()==='primary','Primary first');
 check((await page.locator('[data-machine="1"] option:checked').textContent()).includes('Offline'),'Offline fallback is selectable');
 await page.locator('[data-machine-up="1"]').click();
 check(await page.locator('[data-machine="0"]').inputValue()==='fallback','Reorder changes priority');
 await page.getByRole('button',{name:'Create job',exact:true}).click();
 await page.waitForFunction(()=>document.querySelector('#job-run'));
 check(await page.locator('[data-machine="0"]').inputValue()==='fallback','Priority persists after save');
 await page.reload();await page.waitForSelector('[data-machine="1"]');
 check(await page.locator('[data-machine="1"]').inputValue()==='primary','Priority persists after reload');
 for(const width of [1440,390])for(const theme of ['light','dark']){
  await page.setViewportSize({width,height:950});await page.emulateMedia({colorScheme:theme});
  await page.locator('#job-targets').scrollIntoViewIfNeeded();
  check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'No horizontal overflow');
  await page.screenshot({path:'/tmp/jobs-machines-'+width+'-'+theme+'.png'});
 }
 await page.locator('#job-placement').selectOption('auto');
 await page.getByRole('button',{name:'Save changes',exact:true}).click();
 await page.waitForFunction(()=>document.querySelector('#job-placement')?.value==='auto'&&document.querySelector('.job-panel-subtitle')?.textContent.includes('Revision 2'));
 await page.reload();await page.waitForSelector('#job-placement');
 check(await page.locator('#job-placement').inputValue()==='auto','Restoring automatic persists');
 await page.locator('#job-placement').selectOption('all');
 check(await page.locator('#job-targets').isHidden(),'All machines hides ordered targets');
 check(await page.locator('#job-all-help').isVisible(),'All machines explains offline waiting');
 await page.getByRole('button',{name:'Save changes',exact:true}).click();
 await page.waitForFunction(()=>document.querySelector('.job-panel-subtitle')?.textContent.includes('Revision 3'));
 await page.reload();await page.waitForSelector('#job-placement');
 check(await page.locator('#job-placement').inputValue()==='all','All machines persists after reload');
 await page.locator('#job-placement').scrollIntoViewIfNeeded();
 await page.screenshot({path:'/tmp/jobs-all-machines-mobile.png'});
 await page.getByRole('button',{name:'Run now',exact:true}).click();
 await page.waitForFunction(()=>document.querySelector('#job-history')?.textContent.includes('All machines'));
 await page.getByRole('button',{name:'Refresh run history',exact:true}).click();
 await page.waitForFunction(()=>document.querySelector('#job-history')?.textContent.includes('0 of 1 machines finished'));
 check((await page.locator('#job-history .job-run').count())===2,'History shows the occurrence and machine execution');
 await page.getByRole('button',{name:'Stop current run',exact:true}).click();
 await page.waitForFunction(()=>document.querySelector('#job-stop').hidden);
 check((await page.locator('#job-history').textContent()).includes('cancelled'),'Stop cancels waiting machine execution');
 check(errors.length===0,'No JavaScript errors');
 return {ok:true,checks:'default, offline selection, reorder, save/reload, automatic reset, all machines save/reload/history/stop, desktop/mobile light/dark'};
}
