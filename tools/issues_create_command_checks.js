async page => {
 const checks=[],check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name)};
 await page.goto('http://127.0.0.1:4782/');await page.waitForFunction(()=>model.csrf&&model.project);
 const project=await page.evaluate(async()=>(await api({action:'create',title:'Prompt variable QA',body:'',labels:[]},'Create command QA '+Date.now())).project.id);
 await page.goto('http://127.0.0.1:4782/#project='+encodeURIComponent(project));await page.waitForSelector('.issue-row');await page.locator('#project-settings-trigger').click();await page.waitForFunction(()=>!document.querySelector('#project-prompt').disabled);
 const prompt='/goal Implement {{issue_command}}. If safe-bash fails, report using `{{create_issue_command poe-code}}`.';
 const command="hey-boss issue create --project 'poe-code' --title '<title>' --body '<markdown>'";
 await page.locator('#project-prompt').fill(prompt);await page.waitForFunction(command=>document.querySelector('#project-instructions-preview').textContent.includes(command)&&document.querySelector('#project-instructions-preview').getAttribute('aria-busy')==='false',command);
 const expected=await page.locator('#project-instructions-preview').textContent();check(!expected.includes('{{create_issue_command'),'Variable expands in live preview');check(await page.locator('#project-goal-indicator').isVisible(),'Goal prefix still works');
 await page.locator('#project-prompt').fill(prompt.replace('poe-code','github.com/poe-internal/poe-code'));await page.waitForFunction(()=>document.querySelector('#project-instructions-preview').textContent.includes("--project 'github.com/poe-internal/poe-code'"));check(true,'Full project IDs expand');
 await page.locator('#project-prompt').fill(prompt);await page.waitForFunction(command=>document.querySelector('#project-instructions-preview').textContent.includes(command)&&document.querySelector('#project-instructions-preview').getAttribute('aria-busy')==='false',command);
 await page.locator('#project-settings-form button[type=submit]').click();await page.waitForFunction(()=>!document.querySelector('#project-settings-dialog').open);
 await page.locator('#project-settings-trigger').click();await page.waitForFunction(()=>!document.querySelector('#project-prompt').disabled);check(await page.locator('#project-prompt').inputValue()===prompt,'Template persists unchanged');await page.keyboard.press('Escape');
 const claimed=await page.evaluate(async()=>api({action:'claim',number:1,force:false},model.project.id));check(claimed.instructions===expected,'Claim instructions equal exact preview');
 await page.setViewportSize({width:390,height:844});await page.locator('#project-settings-trigger').click();await page.waitForFunction(()=>!document.querySelector('#project-prompt').disabled);check(await page.evaluate(()=>document.querySelector('#project-settings-dialog').getBoundingClientRect().right<=innerWidth),'Mobile instructions fit viewport');await page.keyboard.press('Escape');
 return {passed:checks.length,checks,project};
}
