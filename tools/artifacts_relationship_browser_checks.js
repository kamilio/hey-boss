async page => {
  await page.reload();
  const fixture=await page.evaluate(async()=>{
    const project=new URLSearchParams(location.hash.slice(1)).get('project');
    const boot=await(await fetch('/api/bootstrap')).json();
    const graph=await(await fetch('/api/mm',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf},body:JSON.stringify({project,operation:{action:'mindmap',operation:{command:'show',body_mode:'preview'}}})})).json();
    return {project,node:graph.nodes.find(n=>n.alias==='release').id,origin:location.origin};
  });
  const title='Map-created plan '+Date.now();
  const updatedTitle='Updated '+title;
  const mapURL=fixture.origin+'/mm#project='+encodeURIComponent(fixture.project)+'&node='+fixture.node;
  await page.goto(mapURL);
  await page.locator('#node-artifacts a').filter({hasText:'Renamed browser plan'}).waitFor();
  await page.locator('#node-artifacts a').filter({hasText:'Renamed browser plan'}).click();
  await page.getByRole('link',{name:'Release',exact:true}).waitFor();
  await page.getByRole('link',{name:'Release',exact:true}).click();
  await page.locator('#node-artifacts a').filter({hasText:'Create artifact'}).waitFor();
  await page.locator('#node-artifacts a').filter({hasText:'Create artifact'}).click();
  const context=await page.evaluate(()=>Object.fromEntries(new URLSearchParams(location.hash.slice(1))));
  if(context.project!==fixture.project||context.node!==fixture.node)throw Error('Map creation lost resource project');
  await page.getByRole('textbox',{name:'Title',exact:true}).fill(title);
  await page.getByRole('textbox',{name:'Markdown',exact:true}).fill('# Shared map document');
  await page.getByRole('button',{name:'Save',exact:true}).click();
  await page.getByRole('link',{name:'Release',exact:true}).waitFor();
  const documentURL=page.url();
  await page.getByRole('link',{name:'Release',exact:true}).click();
  await page.locator('#node-artifacts a').filter({hasText:title}).waitFor();
  await page.getByRole('button',{name:'Unlink '+title,exact:true}).click();
  await page.getByRole('button',{name:'Attach existing',exact:true}).click();
  await page.getByRole('searchbox',{name:'Find artifact to attach',exact:true}).fill(title);
  await page.waitForFunction(()=>document.querySelector('[aria-label="Artifact to attach"]')?.options.length===1);
  await page.getByRole('button',{name:'Attach',exact:true}).click();
  await page.locator('#node-artifacts a').filter({hasText:title}).waitFor();
  await page.locator('#node-artifacts a').filter({hasText:title}).click();
  if(page.url()!==documentURL)throw Error('Reattachment copied the document');
  await page.getByRole('link',{name:'Release',exact:true}).waitFor();
  const identity=await page.evaluate(()=>new URLSearchParams(location.hash.slice(1)).get('artifact'));
  await page.goto(mapURL);
  await page.locator('#node-artifacts a').filter({hasText:title}).waitFor();
  await page.evaluate(async({project,identity,updatedTitle})=>{
    const boot=await(await fetch('/api/bootstrap')).json();
    const response=await fetch('/api/action',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf},body:JSON.stringify({project,operation:{action:'artifact',operation:{command:'edit',id:identity,title:updatedTitle,if_version:1}}})});
    if(!response.ok)throw Error(await response.text());
  },{project:fixture.project,identity,updatedTitle});
  await page.getByRole('button',{name:'Refresh',exact:true}).click();
  await page.locator('#node-artifacts a').filter({hasText:updatedTitle}).waitFor();
  return {shared_issue_map_target:true,automatic_project:true,unlink_preserves_document:true,attach_reuses_identity:true,refresh_updates_references:true};
}
