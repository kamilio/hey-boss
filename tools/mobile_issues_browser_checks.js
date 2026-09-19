// Run through playwright-cli on a paired, isolated mobile hub with synthetic
// bridge token synthetic-fixture-token-000000000000 and a hey-boss project.
async (page) => {
 const origin=await page.evaluate(()=>location.origin),checks=[],errors=[];page.on('pageerror',e=>errors.push(e.message));
 const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 const title='Lost acknowledgment '+Date.now();
 await page.setViewportSize({width:390,height:844});
 await page.locator('#issue-project').selectOption('github.com/kamilio/hey-boss');
 await page.locator('#issue-title').fill('🌍'.repeat(129));
 await page.locator('#issue-description').fill('Full draft\nRetain this description');
 await page.locator('#issue-labels').fill('ready, phone');
 await page.getByRole('button',{name:'Create issue',exact:true}).click();
 await page.getByRole('alert').filter({hasText:'512 bytes'}).waitFor();
 check(await page.locator('#issue-title').inputValue()==='🌍'.repeat(129),'Validation retains title');
 check(await page.locator('#issue-description').inputValue()==='Full draft\nRetain this description','Validation retains description');
 check(await page.locator('#issue-title').isEnabled(),'Validation re-enables editing');
 await page.locator('#issue-title').fill(title);
 let dropped=false;
 await page.route('**/api/issues',async route=>{
  if(route.request().method()==='POST'&&!dropped){dropped=true;await route.fetch();await route.abort('failed');}
  else await route.continue();
 });
 await page.getByRole('button',{name:'Create issue',exact:true}).click();
 await page.getByRole('alert').filter({hasText:'saved on this phone'}).waitFor();
 const saved=await page.evaluate(()=>JSON.parse(localStorage.getItem('hey-boss-issue-draft')));
 check(saved.submitted&&saved.title===title,'Uncertain creation is durably saved');
 check(await page.locator('#issue-title').isDisabled(),'Uncertain payload cannot be changed');
 await page.reload();await page.getByRole('button',{name:'Issues',exact:true}).click();
 await page.waitForFunction(()=>JSON.parse(localStorage.getItem('hey-boss-issue-draft'))?.submitted===false);
 const response=await page.request.get(origin+'/api/issues');const state=await response.json();
 const rows=state.creations.filter(c=>c.title===title);
 check(rows.length===1&&rows[0].requestID===saved.requestID,'Reload retries the original ID without duplicate creation');
 check(rows[0].status==='pending','Offline supervisor leaves accepted creation pending');
 const result=await page.request.post(origin+'/api/bridge/issues/'+saved.requestID+'/result',{headers:{Authorization:'Bearer synthetic-fixture-token-000000000000'},data:{status:'error',error:'Choose another registered project'}});
 check(result.ok(),'Synthetic authoritative error recorded');
 await page.getByRole('button',{name:'Edit saved draft'}).click();
 await page.waitForFunction(title=>document.querySelector('#issue-title').value===title,title);
 check(await page.locator('#issue-description').inputValue()==='Full draft\nRetain this description','Delivery error restores complete original draft');
 check(await page.locator('#issue-labels').inputValue()==='ready, phone','Delivery error restores labels');
 check(await page.evaluate(()=>JSON.parse(localStorage.getItem('hey-boss-issue-draft')).requestID)!==saved.requestID,'Corrected draft gets a new creation ID');
 await page.getByRole('button',{name:'Create issue',exact:true}).click();
 await page.waitForFunction(()=>JSON.parse(localStorage.getItem('hey-boss-issue-draft')).title==='');
 const latest=(await (await page.request.get(origin+'/api/issues')).json()).creations.find(c=>c.title===title&&c.status==='pending');
 await page.request.post(origin+'/api/bridge/issues/'+latest.requestID+'/result',{headers:{Authorization:'Bearer synthetic-fixture-token-000000000000'},data:{status:'synced',number:42}});
 await page.getByText('Synced · #42',{exact:true}).waitFor();
 check(true,'Synced state shows the authoritative issue number');
 check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Phone layout has no horizontal overflow');
 await page.screenshot({path:'output/playwright/issue28-phone.png',fullPage:true});
 await page.emulateMedia({colorScheme:'dark',reducedMotion:'reduce'});
 await page.screenshot({path:'output/playwright/issue28-phone-dark.png',fullPage:true});
 check(errors.length===0,'No JavaScript runtime errors');
 await page.unroute('**/api/issues');
 return {passed:checks.length,checks};
}
