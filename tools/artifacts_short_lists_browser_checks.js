// Deferred short lists must preserve navigation and selection in long documents.
async page => {
  const origin=await page.evaluate(()=>location.origin),project='named:Artifact Performance';
  const boot=await(await page.request.get(origin+'/api/bootstrap')).json();
  const body='# Long reading\n\n'+Array.from({length:1500},(_,i)=>`## Section ${i}\n\nContext for decision ${i}.\n\n- First choice\n- Second choice\n- Keep the useful context\n`).join('\n')+'\nA final **reachable passage** with a source.[^end]\n\n```js\nconst finalMarker = true;\n```\n\n[^end]: Final source explanation.';
  const response=await page.request.post(origin+'/api/action',{headers:{'X-Hey-Boss-CSRF':boot.csrf},data:{project,operation:{action:'artifact',operation:{command:'create',title:'Short list navigation',body}},request_id:await page.evaluate(()=>crypto.randomUUID())}});
  if(!response.ok())throw Error(await response.text());
  const saved=(await response.json()).artifact;
  for(const width of [390,1440]) {
    await page.setViewportSize({width,height:844});
    await page.goto(origin+'/artifacts#project='+encodeURIComponent(project)+'&artifact='+saved.id);await page.reload();
    await page.locator('#artifact-reading').waitFor();
    await page.waitForFunction(()=>!document.querySelector('#artifact-reading').hasAttribute('aria-busy'));
    if(!await page.locator('#artifact-reading').evaluate((reader,html)=>{const canonical=document.createElement('template');canonical.innerHTML=html;return reader.innerHTML===canonical.innerHTML;},saved.body_html))throw Error('Canonical HTML changed');
    const final=page.locator('#artifact-reading strong').filter({hasText:'reachable passage'});
    await final.evaluate(el=>el.scrollIntoView({block:'center'}));
    await page.waitForFunction(()=>{const el=[...document.querySelectorAll('#artifact-reading strong')].find(el=>el.textContent==='reachable passage');const r=el.getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight;});
    const route=page.url();
    await page.locator('#artifact-reading .footnote-reference a').click();
    await page.waitForFunction(()=>{const r=document.querySelector('#artifact-reading .footnote-definition').getBoundingClientRect();return r.top>=0&&r.top<innerHeight;});
    if(page.url()!==route)throw Error('Footnote replaced route');
    await page.locator('#artifact-reading pre code').scrollIntoViewIfNeeded();
    await page.waitForFunction(()=>document.querySelector('#artifact-reading pre code').innerText==='const finalMarker = true;\n');
    await final.evaluate(el=>el.scrollIntoView({block:'center'}));
    await page.evaluate(()=>{const text=[...document.querySelectorAll('#artifact-reading strong')].find(el=>el.textContent==='reachable passage').firstChild;const range=document.createRange();range.selectNodeContents(text);getSelection().removeAllRanges();getSelection().addRange(range);document.querySelector('#artifact-reading').dispatchEvent(new Event('pointerup'));});
    await page.getByRole('button',{name:'Comment on selection',exact:true}).click();
    await page.getByRole('textbox',{name:'Add a comment',exact:true}).fill('Reachable selection '+width);
    await page.getByRole('button',{name:'Comment',exact:true}).click();
    await page.getByText('Reachable selection '+width,{exact:true}).waitFor();
    if(await page.getByText('Outdated selection · discussion preserved',{exact:true}).count())throw Error('Selection anchor lost');
    await page.screenshot({path:'output/playwright/artifact-redesign/'+page.context().browser().browserType().name()+'-short-list-end-'+width+'.png'});
  }
  return {lists:1500,phoneAndDesktop:true,canonicalHtml:true,finalPassage:true,code:true,footnote:true,selection:true};
}
