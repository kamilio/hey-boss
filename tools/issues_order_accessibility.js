// Embed axe-core with prepare_issues_web_accessibility.py before run-code.
async page => {
 await page.reload();await page.waitForSelector('.issue-order-handle');
 await page.evaluate(/* AXE_SOURCE */);
 const reports=[];
 const audit=async name=>{const result=await page.evaluate(async()=>{const r=await axe.run(document,{runOnly:{type:'tag',values:['wcag2a','wcag2aa','wcag21aa','best-practice']}});return {violations:r.violations.map(v=>({id:v.id,impact:v.impact,targets:v.nodes.map(n=>n.target)})),passes:r.passes.length}});reports.push({name,...result});if(result.violations.length)throw Error(JSON.stringify(reports));};
 await page.setViewportSize({width:1440,height:1000});
 await page.emulateMedia({colorScheme: 'light'});
 await audit('desktop-light-order');
 await page.emulateMedia({colorScheme: 'dark'});await audit('desktop-dark-order');
 const handle=page.locator('.issue-order-handle').first();await handle.focus();await audit('focused-keyboard-grip');
 const h=await handle.boundingBox(),target=await page.locator('.issue-row').nth(1).boundingBox();
 await page.mouse.move(h.x+h.width/2,h.y+h.height/2);await page.mouse.down();await page.mouse.move(target.x+80,target.y+target.height-8,{steps:8});
 await page.waitForSelector('.issue-drop-after');await audit('active-drag');await page.keyboard.press('Escape');await page.mouse.up();
 await page.setViewportSize({width:390,height:844});await audit('mobile-dark-order');
 await page.emulateMedia({colorScheme: 'light'});await audit('mobile-light-order');
 await page.setViewportSize({width:320,height:640});await audit('narrow-mobile-order');
 return {states:reports.length,reports};
}
