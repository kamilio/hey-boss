// Run against an isolated Attachment Studio fixture with issues, nodes and artifacts.
// playwright-cli -s=issue57 run-code --filename=tools/attachments_browser_checks.js
async page => {
  const origin=await page.evaluate(()=>location.origin),project='named:Attachment Studio';
  await page.reload();
  const tag=await page.evaluate(()=>String(Date.now())),draggedName=`dragged-${tag}.txt`,retryName=`network-retry-${tag}.txt`;
  const errors=[],checks=[],screenshots=[];
  page.on('pageerror',e=>errors.push(e.message));
  const fixture=await page.evaluate(async project=>{
    const boot=await (await fetch('/api/bootstrap')).json();
    const call=async operation=>(await (await fetch('/api/action',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf},body:JSON.stringify({project,operation})})).json());
    const docs=await call({action:'artifact',operation:{command:'list',archived:false,offset:0}});
    const graph=await call({action:'mindmap',operation:{command:'show'}});
    return {artifact:docs.artifacts[0].id,node:graph.nodes.find(n=>n.alias==='resources').id};
  },project);
  const routes={issue:`/#project=${encodeURIComponent(project)}&issue=1`,artifact:`/artifacts#project=${encodeURIComponent(project)}&artifact=${fixture.artifact}`,node:`/mm#project=${encodeURIComponent(project)}&node=${fixture.node}`};
  const panel=()=>page.locator('.file-attachments:visible');
  const wait=async()=>{await panel().waitFor();await page.waitForFunction(()=>{const root=[...document.querySelectorAll('.file-attachments')].find(r=>r.getBoundingClientRect().height);return root&&!root.querySelector('.attachment-status').textContent.includes('Loading');});if(await panel().locator('.attachment-error:visible').count())throw Error(await panel().locator('.attachment-error').textContent());};
  for(const theme of ['light','dark']) {
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    for(const [width,height] of [[1440,1000],[768,1024],[390,844],[320,740]]) {
      await page.setViewportSize({width,height});
      for(const [kind,route] of Object.entries(routes)) {
        await page.goto(origin+route);await wait();
        if(await panel().locator('li').count()<3)throw Error('Fixture files missing on '+kind);
        await panel().scrollIntoViewIfNeeded();
        if(kind==='node') {
          const info=await panel().locator('.attachment-info').first().boundingBox();
          if(info.width<120)throw Error('Unreadable filename column in inspector');
          await page.locator('#map-details').evaluate(el=>{const file=el.querySelector('.file-attachments');el.scrollTop+=file.getBoundingClientRect().top-el.getBoundingClientRect().top;});
        }
        const bad=await page.evaluate(()=>({page:document.documentElement.scrollWidth>innerWidth+1,files:[...document.querySelectorAll('.attachment-list li')].some(e=>e.scrollWidth>e.clientWidth+1)}));
        if(bad.page||bad.files)throw Error(`Overflow: ${theme}-${width}-${kind} ${JSON.stringify(bad)}`);
        const path=`output/playwright/issue57/${theme}-${width}-${kind}.png`;
        await page.screenshot({path,fullPage:kind!=='node'});screenshots.push(path);checks.push(`${theme}-${width}-${kind}`);
      }
    }
  }
  await page.setViewportSize({width:1440,height:1000});
  for(const [kind,route] of Object.entries(routes)) {
    await page.goto(origin+route);await wait();
    const name=`${kind}-binary.bin`,bytes=[0,10,128,255,42];
    await panel().locator('input[type=file]').setInputFiles(`output/playwright/issue57/${name}`);
    await panel().getByRole('button',{name,exact:true}).waitFor();
    const [download]=await Promise.all([page.waitForEvent('download'),panel().getByRole('button',{name,exact:true}).click()]);if(download.suggestedFilename()!==name)throw Error('Wrong download name');
    const stream=await download.createReadStream(),chunks=[];for await(const chunk of stream)chunks.push(...chunk);
    if(JSON.stringify(chunks)!==JSON.stringify(bytes))throw Error('Downloaded bytes differ');
    await page.waitForFunction(()=>document.querySelector('.file-attachments').dataset.busy==='false');
    await panel().getByRole('button',{name:`Remove ${name}`,exact:true}).click();
    await panel().getByRole('button',{name:'Keep file',exact:true}).click();
    if(await panel().getByRole('button',{name,exact:true}).count()!==1)throw Error('Cancel removed a file');
    await panel().getByRole('button',{name:`Remove ${name}`,exact:true}).click();
    await panel().getByRole('button',{name:'Remove',exact:true}).click();
    await panel().getByRole('button',{name,exact:true}).waitFor({state:'detached'});
    checks.push(`${kind}-upload-download-confirm-remove`);
  }
  await page.goto(origin+routes.issue);await wait();
  const drop=async(name,data='Dragged attachment')=>page.locator('#comment-form .markdown-editor').evaluate((el,{name,data})=>{const dt=new DataTransfer();dt.items.add(new File([data],name,{type:'text/plain'}));el.dispatchEvent(new DragEvent('drop',{bubbles:true,cancelable:true,dataTransfer:dt}));},{name,data});
  await drop(draggedName);await panel().getByRole('button',{name:draggedName,exact:true}).waitFor();checks.push('drag-and-drop');
  const retryIDs=[];let reject=true;
  await page.route('**/api/action',async route=>{
    const value=route.request().postDataJSON();
    if(value.operation?.action==='attachment'&&value.operation.operation.command==='upload'&&value.operation.operation.name===retryName) {
      retryIDs.push(value.request_id);
      if(reject){reject=false;await route.fetch();await route.fulfill({status:503,contentType:'application/json',body:JSON.stringify({ok:false,error:{message:'Synthetic lost acknowledgment'}})});return;}
    }
    await route.continue();
  });
  await drop(retryName);await panel().getByRole('button',{name:'Retry',exact:true}).waitFor();
  await panel().getByRole('button',{name:'Retry',exact:true}).click();
  await panel().getByRole('button',{name:retryName,exact:true}).waitFor();
  if(retryIDs.length!==2||retryIDs[0]!==retryIDs[1])throw Error('Retry changed request ID');
  await page.reload();await wait();if(await panel().getByRole('button',{name:retryName,exact:true}).count()!==1)throw Error('Lost acknowledgment created duplicate files');
  await page.unroute('**/api/action');checks.push('lost-acknowledgment-idempotent-retry');
  await panel().locator('input[type=file]').setInputFiles('output/playwright/issue57/too-large.bin');
  await panel().locator('.attachment-error').filter({hasText:'exceeds 10 MB'}).waitFor();
  if(await panel().getByRole('button',{name:'too-large.bin',exact:true}).count())throw Error('Oversized file uploaded');checks.push('oversized-file-error');
  await page.screenshot({path:'output/playwright/issue57/oversized-error.png',fullPage:true});
  if(errors.length)throw Error(errors.join('\n'));
  return {checks,screenshots,scriptErrors:errors};
}
