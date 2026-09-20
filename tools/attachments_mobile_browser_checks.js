// Paired mobile UI, using serve_attachments_mobile_fixture.mjs and its native RPC.
async page => {
 const origin=await page.evaluate(()=>location.origin),project='named:Attachment Studio',errors=[],checks=[];
 page.on('pageerror',e=>errors.push(e.message));
 await page.goto(origin+`/artifacts#project=${encodeURIComponent(project)}`);await page.reload();
 await page.locator('.artifact-row').first().waitFor();
 const fixture=await page.evaluate(async project=>{
  const context={project};
  const docs=await HeyBossArtifacts.rpc(context,{action:'artifact',operation:{command:'list',archived:false,offset:0}},true);
  const graph=await HeyBossArtifacts.rpc(context,{action:'mindmap',operation:{command:'show'}},true);
  return {artifact:docs.artifacts[0].id,node:graph.nodes.find(n=>n.alias==='resources').id};
 },project);
 const routes={artifact:`/artifacts#project=${encodeURIComponent(project)}&artifact=${fixture.artifact}`,issue:`/project-resource#project=${encodeURIComponent(project)}&issue=1`,node:`/project-resource#project=${encodeURIComponent(project)}&node=${fixture.node}`};
 const panel=()=>page.locator('.file-attachments:visible');
 async function wait(kind){await page.getByRole('heading',{name:kind==='node'?'Release resources':kind==='issue'?'Prepare the autumn release':'Autumn release checklist',exact:true}).waitFor();await panel().locator('.attachment-name').first().waitFor();if(await panel().locator('.attachment-error:visible').count())throw Error(await panel().locator('.attachment-error').textContent());}
 for(const theme of ['light','dark']) {
  await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
  for(const [width,height] of [[390,844],[320,740]]) {
   await page.setViewportSize({width,height});
   for(const [kind,route] of Object.entries(routes)) {
    await page.goto(origin+route);await wait(kind);
    if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1))throw Error(`Mobile overflow: ${theme}-${width}-${kind}`);
    await page.screenshot({path:`output/playwright/issue57/mobile-${theme}-${width}-${kind}.png`,fullPage:true});checks.push(`${theme}-${width}-${kind}`);
   }
  }
 }
 for(const [kind,route] of Object.entries(routes)) {
  await page.goto(origin+route);await wait(kind);const name=`mobile-${kind}-binary.bin`;
  await panel().locator('input[type=file]').setInputFiles(`output/playwright/issue57/${name}`);
  await panel().getByRole('button',{name,exact:true}).waitFor();
  const [download]=await Promise.all([page.waitForEvent('download'),panel().getByRole('button',{name,exact:true}).click()]);
  if(download.suggestedFilename()!==name)throw Error('Mobile filename changed');
  const stream=await download.createReadStream(),bytes=[];for await(const chunk of stream)bytes.push(...chunk);
  if(JSON.stringify(bytes)!==JSON.stringify([0,10,128,255,42]))throw Error('Mobile bytes differ');
  await page.waitForFunction(()=>document.querySelector('.file-attachments').dataset.busy==='false');
  await panel().getByRole('button',{name:`Remove ${name}`,exact:true}).click();await panel().getByRole('button',{name:'Remove',exact:true}).click();
  await panel().getByRole('button',{name,exact:true}).waitFor({state:'detached'});checks.push(`${kind}-paired-upload-download-remove`);
 }
 if(errors.length)throw Error(errors.join('\n'));
 return {checks,scriptErrors:errors};
}
