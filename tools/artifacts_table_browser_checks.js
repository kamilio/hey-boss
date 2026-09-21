// Read-only regression checks for issue 88 on any open artifact. The synthetic
// DOM fixture keeps the matrix stable while saved documents are being edited.
async page => {
  const checks=[],errors=[];
  page.on('pageerror',e=>errors.push(e.message));
  await page.locator('#artifact-reading').waitFor();
  await page.waitForFunction(()=>!document.querySelector('#artifact-reading').hasAttribute('aria-busy'));
  const original=await page.locator('#artifact-reading').innerHTML();
  try {
    await page.locator('#artifact-reading').evaluate(el=>{el.innerHTML='<h1>PR merge order</h1><p>Keep verified prerequisites ahead of their dependent changes. Review the latest evidence before each merge.</p><table><thead><tr><th class="markdown-align-right">Order</th><th>PR</th><th>What it does</th><th>Current blocker</th><th>Order notes</th></tr></thead><tbody>'+Array.from({length:8},(_,i)=>'<tr><td class="markdown-align-right">'+(i+1)+'</td><td><a href="https://example.com/pr/'+i+'">#1492'+i+'</a></td><td>Keep accepted chat replies safe</td><td>Waiting for two requested reviews</td><td>'+('Keep prerequisites ahead of dependent changes. Verify current reviews, checks and conflicts before merging. ').repeat(i%3+1)+'</td></tr>').join('')+'</tbody></table>';});
    for(const theme of ['light','dark']) {
      await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
      for(const width of [1920,1440,1024,820,390,320]) {
        await page.setViewportSize({width,height:1000});
        const reader=page.locator('#artifact-reading');
        await reader.waitFor();
        await page.waitForFunction(()=>!document.querySelector('#artifact-reading').hasAttribute('aria-busy'));
        const table=reader.locator('table').first();
        await table.evaluate(el=>el.scrollIntoView({block:'start'}));
        const metrics=await reader.evaluate(el=>{
          const table=el.querySelector('table');
          const cells=[...table.querySelector('tbody tr').children];
          return {reader:el.getBoundingClientRect().width,pageOverflow:document.documentElement.scrollWidth>innerWidth+1,
            tableOverflow:table.scrollWidth>table.clientWidth+1,
            prWraps:cells[1].querySelector('a').getClientRects().length>1,
            headerWraps:[...table.querySelectorAll('th')].some(c=>c.scrollHeight>48),
            verticalAlign:getComputedStyle(cells[4]).verticalAlign};
        });
        if(metrics.pageOverflow)throw Error('Page overflow at '+width);
        if(width===1920&&metrics.reader<1600)throw Error('Wide screen wasted: reader is '+metrics.reader+'px');
        if(width===1440&&metrics.reader<1200)throw Error('Desktop reader remains constrained');
        if(metrics.prWraps||metrics.headerWraps)throw Error('Table labels break at '+width);
        if(metrics.verticalAlign!=='top')throw Error('Long rows do not align at the top');
        if(width<=390&&!metrics.tableOverflow)throw Error('Phone table should scroll within the reader');
        if(width>=1440&&metrics.tableOverflow)throw Error('Desktop table unnecessarily scrolls');
        if(metrics.tableOverflow) {
          await table.evaluate(el=>{el.scrollLeft=el.scrollWidth;});
          if(await table.evaluate(el=>el.scrollLeft)<=0)throw Error('Table cannot scroll');
          await table.evaluate(el=>{el.scrollLeft=0;});
        }
        await page.screenshot({path:'output/playwright/issue88/'+theme+'-'+width+'.png'});
        checks.push({theme,width,...metrics});
      }
    }
    await page.setViewportSize({width:1920,height:1080});
    await page.locator('#artifact-comments-toggle').click();
    if(!await page.locator('#artifact-comments').isVisible())throw Error('Comments unavailable');
    if(await page.locator('#artifact-reading').evaluate(el=>el.getBoundingClientRect().width)<1200)throw Error('Comments squeeze the wide reader');
    await page.screenshot({path:'output/playwright/issue88/comments-desktop.png'});
    await page.locator('#artifact-comments-close').click();
    // Exercise neighboring layouts in the browser without changing saved content.
    await page.locator('#artifact-reading').evaluate(el=>{el.innerHTML='<p>Ordinary prose keeps a comfortable reading width.</p>';});
    if(await page.locator('#artifact-reading').evaluate(el=>el.getBoundingClientRect().width)>820)throw Error('Prose reader widened');
    await page.locator('#artifact-reading').evaluate(el=>{el.innerHTML='<p>Summary</p><table><thead><tr><th>Item</th><th>Notes</th></tr></thead><tbody><tr><td class="markdown-align-right">42</td><td>Read the <a href="https://example.com">source</a> and keep surrounding text able to wrap.</td></tr></tbody></table>';});
    for(const width of [1920,320]) {
      await page.setViewportSize({width,height:1000});
      if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1))throw Error('Short table page overflow');
      if(await page.locator('#artifact-reading td').first().evaluate(el=>getComputedStyle(el).textAlign)!=='right')throw Error('Numeric alignment lost');
      if(await page.locator('#artifact-reading td').nth(1).evaluate(el=>getComputedStyle(el).whiteSpace)!=='normal')throw Error('Linked sentence cannot wrap');
    }
    await page.locator('#artifact-reading td').nth(1).evaluate(el=>{el.innerHTML='<a href="https://example.com">https://example.com/'+ 'long-path'.repeat(50)+'</a>';});
    if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1))throw Error('Long URL escaped table scroller');
  } finally {
    await page.locator('#artifact-reading').evaluate((el,html)=>{el.innerHTML=html;},original);
  }
  if(errors.length)throw Error(errors.join('\n'));
  return {checks,comments:true,proseWidth:true,shortTables:true,longURLs:true,numericAlignment:true,scriptErrors:errors};
}
