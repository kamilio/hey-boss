// Run with playwright-cli run-code against an isolated issue web server.
async page => {
  const origin=await page.evaluate(()=>location.origin), project='named:External artifact links';
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  const boot=await(await page.request.get(origin+'/api/bootstrap')).json();
  const action=async operation=>{
    const r=await page.request.post(origin+'/api/action',{headers:{'X-Hey-Boss-CSRF':boot.csrf},data:{project,operation:{action:'artifact',operation},request_id:await page.evaluate(()=>crypto.randomUUID())}});
    if(!r.ok())throw Error(await r.text());return r.json();
  };
  const body='# Reference notes\n\n[External source](https://example.org/reference) · <https://example.org/automatic> · https://example.org/bare\n\n[Protocol relative](//example.org/relative) · [Local library](/artifacts) · [Same site]('+origin+'/artifacts) · [Email](mailto:hello@example.org)\n\nA footnote.[^note]\n\n[^note]: Keep the document open.';
  const created=await action({command:'create',title:'External link navigation',body}),id=created.artifact.id;
  await action({command:'comment',id,body:'[Discussion source](https://example.org/discussion)'});
  await page.context().route('https://example.org/**',route=>route.fulfill({contentType:'text/html',body:'<h1>External source</h1>'}));
  const route=origin+'/artifacts#project='+encodeURIComponent(project)+'&artifact='+encodeURIComponent(id);
  const checkLinks=async root=>{
    const links=await root.locator('a').evaluateAll(nodes=>nodes.map(a=>({href:a.getAttribute('href'),target:a.target,rel:a.rel,external:['http:','https:'].includes(a.protocol)&&a.origin!==location.origin})));
    for(const a of links){
      const external=a.external;
      if(external&&(a.target!=='_blank'||!a.rel.split(' ').includes('noopener')||!a.rel.split(' ').includes('noreferrer')))throw Error('External link lacks a safe new tab: '+a.href);
      if(!external&&a.target==='_blank')throw Error('Internal or email link opens a new tab: '+a.href);
    }
    if(!links.some(a=>a.target==='_blank'))throw Error('Missing external links');
  };
  const screenshots=[];
  for(const theme of ['light','dark'])for(const width of [1440,390,320]){
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    await page.setViewportSize({width,height:900});await page.goto(route);await page.reload();
    const reader=page.locator('#artifact-reading');await reader.getByRole('link',{name:'External source',exact:true}).waitFor();
    await checkLinks(reader);
    const link=reader.getByRole('link',{name:'External source',exact:true});
    const popupPromise=page.waitForEvent('popup');await link.click();const popup=await popupPromise;
    await popup.waitForLoadState();
    if(await popup.evaluate(()=>window.opener!==null))throw Error('New tab retains opener');
    if(page.url()!==route)throw Error('External link replaced the artifact');await popup.close();
    await reader.locator('.footnote-reference a').click();
    if(page.url()!==route)throw Error('Footnote replaced the artifact');
    await page.locator('#artifact-comments-toggle').click();await checkLinks(page.locator('#artifact-threads'));
    if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1))throw Error('Layout overflows');
    const path='output/playwright/issue98/'+theme+'-'+width+'.png';await page.screenshot({path,fullPage:true});screenshots.push(path);
    await page.locator('#artifact-edit').click();await page.locator('#artifact-preview').click();
    await page.locator('#artifact-edit-preview a').first().waitFor();await checkLinks(page.locator('#artifact-edit-preview'));
    await page.locator('#artifact-cancel').click();
  }
  // Large documents use a separate streaming rendering path.
  await action({command:'edit',id,if_version:1,body:body+'\n\n'+('Paragraph with [another source](https://example.org/large).\n\n'.repeat(900))});
  await page.goto(route);await page.reload();await page.locator('#artifact-reading[aria-busy]').waitFor({state:'detached'});
  await page.getByRole('link',{name:'another source',exact:true}).nth(899).waitFor();await checkLinks(page.locator('#artifact-reading'));
  const link=page.getByRole('link',{name:'External source',exact:true});await link.focus();
  const popupPromise=page.waitForEvent('popup');await page.keyboard.press('Enter');const popup=await popupPromise;await popup.close();
  if(errors.length)throw Error(errors.join('\n'));
  return {checks:'reader, preview, comments, autolinks, footnotes, streaming, keyboard, safe popups',screenshots,scriptErrors:errors};
}
