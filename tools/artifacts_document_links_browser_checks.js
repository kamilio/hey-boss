// Internal document links scroll inside the reader without replacing its route.
async page => {
  const origin=await page.evaluate(()=>location.origin),project='named:Artifact Performance';
  const boot=await(await page.request.get(origin+'/api/bootstrap')).json();
  const body='# Notes\n\nA footnote.[^source]\n\n'+Array.from({length:50},(_,i)=>'Paragraph '+i+' with some useful context.\n').join('\n')+'\n[^source]: Source explanation.';
  const response=await page.request.post(origin+'/api/action',{headers:{'X-Hey-Boss-CSRF':boot.csrf},data:{project,operation:{action:'artifact',operation:{command:'create',title:'Document navigation',body}},request_id:await page.evaluate(()=>crypto.randomUUID())}});
  const id=(await response.json()).artifact.id;
  await page.setViewportSize({width:390,height:844});await page.goto(origin+'/artifacts#project='+encodeURIComponent(project)+'&artifact='+id);await page.reload();
  await page.locator('#artifact-reading').waitFor();const route=page.url();
  await page.locator('#artifact-reading .footnote-reference a').click();
  if(page.url()!==route)throw Error('Footnote replaced the artifact route');
  await page.locator('#artifact-reading .footnote-definition').waitFor();
  if(!await page.locator('#artifact-reading .footnote-definition').evaluate(el=>{const r=el.getBoundingClientRect();return r.top>=0&&r.top<innerHeight;}))throw Error('Footnote did not scroll into view');
  await page.screenshot({path:'output/playwright/artifact-redesign/'+page.context().browser().browserType().name()+'-footnote-navigation.png'});
  return {routeRetained:true,footnoteReachable:true};
}
