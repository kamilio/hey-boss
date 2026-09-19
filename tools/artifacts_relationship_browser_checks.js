async page => {
  const fixture=await page.evaluate(async()=>{
    const project=new URLSearchParams(location.hash.slice(1)).get('project');
    const boot=await(await fetch('/api/bootstrap')).json();
    const graph=await(await fetch('/api/mm',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf},body:JSON.stringify({project,operation:{action:'mindmap',operation:{command:'show',body_mode:'preview'}}})})).json();
    return {project,node:graph.nodes.find(n=>n.alias==='release').id,origin:location.origin};
  });
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
  await page.getByRole('textbox',{name:'Title',exact:true}).fill('Map-created plan');
  await page.getByRole('textbox',{name:'Markdown',exact:true}).fill('# Shared map document');
  await page.getByRole('button',{name:'Save',exact:true}).click();
  await page.getByRole('link',{name:'Release',exact:true}).waitFor();
  const documentURL=page.url();
  await page.getByRole('link',{name:'Release',exact:true}).click();
  await page.locator('#node-artifacts a').filter({hasText:'Map-created plan'}).waitFor();
  await page.getByRole('button',{name:'Unlink Map-created plan',exact:true}).click();
  await page.getByRole('button',{name:'Attach existing',exact:true}).click();
  await page.getByRole('searchbox',{name:'Find artifact to attach',exact:true}).fill('Map-created');
  await page.waitForFunction(()=>document.querySelector('[aria-label="Artifact to attach"]')?.options.length===1);
  await page.getByRole('button',{name:'Attach',exact:true}).click();
  await page.locator('#node-artifacts a').filter({hasText:'Map-created plan'}).waitFor();
  await page.locator('#node-artifacts a').filter({hasText:'Map-created plan'}).click();
  if(page.url()!==documentURL)throw Error('Reattachment copied the document');
  await page.getByRole('link',{name:'Release',exact:true}).waitFor();
  return {shared_issue_map_target:true,automatic_project:true,unlink_preserves_document:true,attach_reuses_identity:true};
}
