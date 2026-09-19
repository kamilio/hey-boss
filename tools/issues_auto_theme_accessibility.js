// Embed axe-core before running through playwright-cli.
async page => {
 await page.reload();await page.waitForSelector('.markdown-alert-warning');await page.evaluate(/* AXE_SOURCE */);
 const reports=[];
 const audit=async name=>{const result=await page.evaluate(async()=>{const r=await axe.run(document,{runOnly:{type:'tag',values:['wcag2a','wcag2aa','wcag21aa','best-practice']}});return {violations:r.violations.map(v=>({id:v.id,impact:v.impact,targets:v.nodes.map(n=>n.target)})),passes:r.passes.length}});reports.push({name,...result});if(result.violations.length)throw Error(JSON.stringify(reports));};
 for(const [name,width,height] of [['desktop',1440,1000],['mobile',390,844]]){
  await page.setViewportSize({width,height});for(const colorScheme of ['light','dark']){await page.emulateMedia({colorScheme});await audit(name+'-'+colorScheme+'-markdown');}
 }
 await page.getByRole('button',{name:'Edit',exact:true}).click();for(const colorScheme of ['light','dark']){await page.emulateMedia({colorScheme});await audit('mobile-'+colorScheme+'-editor');}await page.keyboard.press('Escape');
 return {states:reports.length,reports};
}
