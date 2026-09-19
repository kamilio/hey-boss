async page => {
 await page.reload();await page.waitForFunction(()=>model.csrf&&model.project);
 const origin=await page.evaluate(()=>location.origin),project=await page.evaluate(async()=>{const r=await api({action:'create',title:'Ship the feature',body:'## Plan\n\nA polished parent issue.',labels:['ready']},'Subtask accessibility '+crypto.randomUUID());const p=r.project.id;await api({action:'create_subtask',number:1,title:'Implement the API',body:'## API',labels:['backend'],at_top:false,if_version:null},p);await api({action:'create_subtask',number:1,title:'Design the interface',body:'## Design',labels:[],at_top:false,if_version:null},p);await api({action:'close',number:2,force:false,comment:null},p);await api({action:'add_pull_request',number:2,url:'https://github.com/example/hey-boss/pull/42'},p);await api({action:'assign_boss',number:3,force:false},p);return p});
 await page.goto(origin+'/#project='+encodeURIComponent(project)+'&issue=1');await page.waitForFunction(()=>model.detail?.subtasks?.length===2);
 await page.evaluate(/* AXE_SOURCE */);
 const reports=[],audit=async name=>{const r=await page.evaluate(async()=>{const r=await axe.run(document,{runOnly:{type:'tag',values:['wcag2a','wcag2aa','wcag21aa','best-practice']}});return {violations:r.violations.map(v=>({id:v.id,impact:v.impact,nodes:v.nodes.map(n=>({target:n.target,summary:n.failureSummary}))})),passes:r.passes.length}});reports.push({name,...r});if(r.violations.length)throw Error(name+': '+JSON.stringify(r.violations))};
 const browser=page.context().browser().browserType().name();
 await page.setViewportSize({width:1440,height:1000});await page.emulateMedia({colorScheme:'light'});await audit('parent-desktop-light');await page.screenshot({path:'output/playwright/subtasks-'+browser+'-light.png'});
 await page.emulateMedia({colorScheme:'dark'});await audit('parent-desktop-dark');await page.screenshot({path:'output/playwright/subtasks-'+browser+'-dark.png'});
 await page.locator('[data-add-existing-subtask]').click();await page.waitForSelector('[data-existing-subtask="2"]');await audit('picker-desktop-dark');await page.screenshot({path:'output/playwright/subtasks-'+browser+'-picker.png'});await page.keyboard.press('Escape');
 await page.locator('[data-create-subtask]').click();await audit('child-editor-desktop');await page.keyboard.press('Escape');
 await page.locator('#subtask-list .subtask-title').first().click();await page.waitForFunction(()=>model.detail?.issue.number===2);await audit('child-breadcrumb');
 await page.locator('.parent-breadcrumb a').click();await page.waitForFunction(()=>model.detail?.issue.number===1);
 await page.setViewportSize({width:320,height:844});await audit('parent-mobile-dark');await page.screenshot({path:'output/playwright/subtasks-'+browser+'-mobile.png'});
 await page.locator('[data-add-existing-subtask]').click();await page.waitForSelector('[data-existing-subtask="2"]');await audit('picker-mobile-dark');await page.keyboard.press('Escape');
 await page.emulateMedia({colorScheme:'light'});await audit('parent-mobile-light');await page.setViewportSize({width:1440,height:1000});
 return {passed:reports.length,reports,project};
}
