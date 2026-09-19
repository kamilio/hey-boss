// Only project instructions belong in the web UI; workers are CLI-only.
async page => {
 const reports=[];await page.goto('http://127.0.0.1:4782/#project=named%3AWorker%20QA&issue=5');await page.reload();
 await page.evaluate(/* AXE_SOURCE */);
 const audit=async name=>{const violations=await page.evaluate(async()=>{const r=await axe.run(document,{runOnly:{type:'tag',values:['wcag2a','wcag2aa','wcag21aa','best-practice']}});return r.violations.map(v=>({id:v.id,nodes:v.nodes.map(n=>n.target)}));});reports.push({name,violations});};
 await page.setViewportSize({width:1440,height:1000});await page.emulateMedia({colorScheme: 'light'});
 await page.locator('#project-settings-trigger').click();await page.waitForFunction(()=>document.querySelector('#project-instructions-preview').textContent.length>0);await audit('desktop-light-instructions');
 await page.locator('#project-settings-close').click();await audit('desktop-light-pr-links');await page.emulateMedia({colorScheme: 'dark'});await page.locator('#project-settings-trigger').click();await page.waitForFunction(()=>document.querySelector('#project-instructions-preview').textContent.length>0);await audit('desktop-dark-instructions');
 await page.setViewportSize({width:390,height:844});await audit('mobile-dark-instructions');await page.keyboard.press('Escape');await audit('mobile-dark-pr-links');await page.goto('http://127.0.0.1:4782/#project=named%3AWorker%20QA');await page.waitForSelector('.issue-pr-link');await audit('mobile-dark-pr-list');await page.setViewportSize({width:1440,height:1000});await page.emulateMedia({colorScheme: 'light'});await audit('desktop-light-pr-list');return reports;
}
