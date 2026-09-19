// Inject axe-core with tools/prepare_issues_web_accessibility.py before running through playwright-cli.
async (page) => {
await page.goto('http://127.0.0.1:4782/#project=github.com%2Fexample%2Fhey-boss');
await page.reload();
await page.getByRole('link',{name:'Reconnect automatically after waking from sleep',exact:true}).waitFor();
await page.evaluate(/* AXE_SOURCE */);
const reports=[];
const audit=async name=>{const r=await page.evaluate(async()=>{const r=await axe.run(document,{runOnly:{type:'tag',values:['wcag2a','wcag2aa','wcag21aa','best-practice']}});return {violations:r.violations.map(v=>({id:v.id,impact:v.impact,nodes:v.nodes.map(n=>({target:n.target,summary:n.failureSummary}))})),passes:r.passes.length,incomplete:r.incomplete.map(v=>v.id)}});reports.push({name,...r});};
await page.setViewportSize({width:1440,height:1000});
await page.emulateMedia({colorScheme: 'light'});
await audit('desktop-light-list');
await page.emulateMedia({colorScheme: 'dark'});
await audit('desktop-dark-list');
await page.getByRole('link',{name:'Reconnect automatically after waking from sleep',exact:true}).click();
await page.getByRole('button',{name:'Edit',exact:true}).waitFor();
await audit('desktop-dark-detail');
await page.emulateMedia({colorScheme: 'light'});
await audit('desktop-light-detail');
await page.getByRole('button',{name:'Edit',exact:true}).click();
await audit('desktop-editor');
await page.keyboard.press('Escape');
await page.setViewportSize({width:390,height:844});
await audit('mobile-detail');
await page.locator('#project-trigger').click();
await audit('mobile-project-switcher');
await page.keyboard.press('Escape');
await page.getByRole('button',{name:'All issues',exact:true}).click();
await page.getByRole('link',{name:'Reconnect automatically after waking from sleep',exact:true}).waitFor();
await audit('mobile-light-list');
await page.emulateMedia({colorScheme: 'dark'});
await audit('mobile-dark-list');
await page.locator('#new-issue').click();
await audit('mobile-dark-editor');
await page.keyboard.press('Escape');
await page.emulateMedia({colorScheme: 'light'});
await page.setViewportSize({width:1440,height:1000});
return reports;
}
