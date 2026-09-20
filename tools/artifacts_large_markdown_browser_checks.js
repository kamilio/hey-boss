// Large Markdown semantics and discussion checks against an isolated store.
async page => {
  const origin=await page.evaluate(()=>location.origin),project='named:Artifact Performance';
  const boot=await(await page.request.get(origin+'/api/bootstrap')).json();
  const body='# Large document\n\n'+Array.from({length:2000},(_,i)=>`${i+3}. Checklist item ${i}`).join('\n')+'\n\n| Item | Decision |\n| --- | --- |\n'+Array.from({length:1000},(_,i)=>`| Row ${i} | Keep **context** |`).join('\n')+'\n\nA final **selected passage** with a footnote.[^note]\n\n[^note]: Keep the explanation.\n\n```js\nconst safe = "<script>";\n```';
  const response=await page.request.post(origin+'/api/action',{headers:{'X-Hey-Boss-CSRF':boot.csrf},data:{project,operation:{action:'artifact',operation:{command:'create',title:'Large Markdown semantics',body}},request_id:await page.evaluate(()=>crypto.randomUUID())}});
  if(!response.ok())throw Error(await response.text());
  const saved=(await response.json()).artifact,id=saved.id;
  await page.setViewportSize({width:390,height:844});
  await page.goto(origin+'/artifacts#project='+encodeURIComponent(project)+'&artifact='+id);await page.reload();
  await page.locator('#artifact-reading').waitFor();
  await page.waitForFunction(()=>!document.querySelector('#artifact-reading').hasAttribute('aria-busy'));
  if(!await page.locator('#artifact-reading').evaluate((reader,html)=>{const canonical=document.createElement('template');canonical.innerHTML=html;return reader.innerHTML===canonical.innerHTML;},saved.body_html))throw Error('Progressive rendering changed canonical HTML');
  if(await page.locator('#artifact-reading ol > li').count()!==2000)throw Error('Large ordered list lost items');
  if(await page.locator('#artifact-reading ol').getAttribute('start')!=='3')throw Error('Ordered list numbering changed');
  if(await page.locator('#artifact-reading tbody tr').count()!==1000)throw Error('Large table lost rows');
  await page.locator('#artifact-reading pre code').scrollIntoViewIfNeeded();
  await page.waitForFunction(()=>document.querySelector('#artifact-reading pre code').innerText==='const safe = "<script>";\n');
  if(await page.locator('#artifact-reading pre code').innerText()!=='const safe = "<script>";\n')throw Error('Code text was changed');
  if(await page.locator('#artifact-reading .footnote-reference').count()!==1)throw Error('Footnote reference was lost');
  await page.locator('#artifact-reading strong').filter({hasText:'selected passage'}).scrollIntoViewIfNeeded();
  await page.waitForFunction(()=>[...document.querySelectorAll('#artifact-reading strong')].some(el=>el.innerText==='selected passage'));
  await page.evaluate(()=>{
    const text=[...document.querySelectorAll('#artifact-reading strong')].find(el=>el.textContent==='selected passage').firstChild;
    const range=document.createRange();range.selectNodeContents(text);const selection=getSelection();selection.removeAllRanges();selection.addRange(range);document.querySelector('#artifact-reading').dispatchEvent(new Event('pointerup'));
  });
  await page.getByRole('button',{name:'Comment on selection',exact:true}).click();
  await page.getByRole('textbox',{name:'Add a comment',exact:true}).fill('Selection from the completed renderer');
  await page.getByRole('button',{name:'Comment',exact:true}).click();
  await page.getByText('Selection from the completed renderer',{exact:true}).waitFor();
  if(await page.locator('#artifact-reading ol > li').count()!==2000||await page.locator('#artifact-reading').getAttribute('aria-busy'))throw Error('Comment restarted the large document rendering');
  if(await page.getByText('Outdated selection · discussion preserved',{exact:true}).count())throw Error('Large document selection anchor became outdated');
  return {id,orderedItems:2000,tableRows:1000,numberingPreserved:true,codePreserved:true,footnotesPreserved:true,selectionPreserved:true,commentPreservesRenderedBody:true};
}
