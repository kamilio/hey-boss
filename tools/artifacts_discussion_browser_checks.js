// Run against an isolated issue store: creates its own document and discussion.
async page => {
  const origin=await page.evaluate(()=>location.origin);
  const boot=await(await page.request.get(origin+'/api/bootstrap')).json();
  const project='named:Artifact Studio',checks=[];
  const action=async operation=>{
    const r=await page.request.post(origin+'/api/action',{headers:{'X-Hey-Boss-CSRF':boot.csrf},data:{project,operation:{action:'artifact',operation},request_id:await page.evaluate(()=>crypto.randomUUID())}});
    if(!r.ok())throw Error(await r.text());return r.json();
  };
  const created=await action({command:'create',title:'Discussion draft check',body:'# Decisions\n\nA discussion worth keeping.'});
  const id=created.artifact.id;
  await action({command:'comment',id,body:'Can we review this decision?'});
  await page.setViewportSize({width:1440,height:1000});
  await page.goto(origin+'/artifacts#project='+encodeURIComponent(project)+'&artifact='+id);
  await page.reload();await page.locator('#artifact-reading').waitFor();
  await page.locator('.artifact-reply summary').click();
  await page.getByRole('textbox',{name:'Reply to comment',exact:true}).fill('My unfinished reply');
  await page.getByRole('button',{name:'Resolve',exact:true}).click();
  await page.locator('details.artifact-thread > summary').click();
  await page.locator('.artifact-reply summary').click();
  if(await page.getByRole('textbox',{name:'Reply to comment',exact:true}).inputValue()!=='My unfinished reply')throw Error('Resolving a discussion lost its reply draft');
  checks.push('Reply draft survives resolving its discussion');
  await page.reload();await page.locator('#artifact-reading').waitFor();
  await page.locator('details.artifact-thread > summary').click();
  await page.locator('.artifact-reply summary').click();
  if(await page.getByRole('textbox',{name:'Reply to comment',exact:true}).inputValue()!=='My unfinished reply')throw Error('Reload lost the reply draft');
  checks.push('Reply draft survives reload');
  await page.getByRole('button',{name:'Reply',exact:true}).click();
  await page.locator('details.artifact-thread > summary').click();
  await page.getByText('My unfinished reply',{exact:true}).waitFor();
  await page.getByRole('button',{name:'Reopen thread',exact:true}).click();
  await page.locator('.artifact-reply summary').click();
  if(await page.getByRole('textbox',{name:'Reply to comment',exact:true}).inputValue()!=='')throw Error('Published reply remains in the draft composer');
  checks.push('Publishing clears only the posted reply draft');
  await page.getByRole('textbox',{name:'Reply to comment',exact:true}).fill('Reply after an interrupted acknowledgment');
  let dropped=false;
  await page.route('**/api/action',async route=>{
    const operation=route.request().postDataJSON()?.operation?.operation;
    if(!dropped&&operation?.command==='comment'&&operation.parent) {
      const response=await route.fetch();if(!response.ok())throw Error(await response.text());
      dropped=true;await route.abort('failed');
    } else await route.continue();
  });
  await page.getByRole('button',{name:'Reply',exact:true}).click();
  await page.getByText('Reply pending · retry after reconnecting',{exact:true}).waitFor();
  await page.unroute('**/api/action');
  await page.reload();await page.locator('#artifact-reading').waitFor();
  await page.locator('.artifact-reply summary').click();
  if(!await page.getByRole('textbox',{name:'Reply to comment',exact:true}).evaluate(el=>el.readOnly))throw Error('Uncertain reply draft can be changed before retry');
  await page.getByRole('button',{name:'Reply',exact:true}).click();
  await page.getByText('Reply saved',{exact:true}).waitFor();
  if(await page.getByText('Reply after an interrupted acknowledgment',{exact:true}).count()!==1)throw Error('Retry created a duplicate reply');
  checks.push('Interrupted reply retries preserve identity and avoid duplicates after reload');
  return {checks,id};
}
