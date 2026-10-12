// Run against a disposable real store seeded with the six Preview Studio documents.
(async (page,base,project,screenshots) => {
  const checks=[],check=(condition,name)=>{if(!condition)throw Error(name);checks.push(name);};
  const library=base+'/artifacts#'+new URLSearchParams({project});
  await page.goto(library);
  await page.locator('.artifact-row-preview').first().waitFor();
  check(await page.locator('.artifact-row').count()===6,'All six documents rendered');
  check(await page.locator('.artifact-row-preview').filter({hasText:'Bring the work that matters'}).count()===1,'Real Markdown excerpt is visible');
  check(await page.locator('.artifact-row-preview img').count()===0,'Preview markup stays inert text');
  check(await page.locator('.artifact-row-preview.is-empty').count()===1,'Empty document has a useful fallback');
  for(const theme of ['light','dark']) {
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    for(const [width,height] of [[1440,1100],[820,1180],[390,844],[320,740]]) {
      await page.setViewportSize({width,height});
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),`${theme} ${width}: no page overflow`);
      check(await page.locator('.artifact-row-preview').evaluateAll(els=>els.every(el=>el.clientHeight<=parseFloat(getComputedStyle(el).lineHeight)*3+1)),`${theme} ${width}: excerpts stay within three lines`);
      check(await page.locator('.artifact-row-actions button').evaluateAll(els=>els.every(el=>{const r=el.getBoundingClientRect();return r.width>=44&&r.height>=44&&r.right<=innerWidth&&r.left>=0;})),`${theme} ${width}: every action has a reachable touch target`);
      await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
      await page.screenshot({path:`${screenshots}/${theme}-${width}.png`,fullPage:true,animations:'disabled'});
      if(width===320){
        await page.locator('.artifact-row').last().scrollIntoViewIfNeeded();
        check(await page.locator('.artifact-row').last().evaluate(el=>{const r=el.getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight;}),`${theme}: last phone card scrolls into view`);
        await page.screenshot({path:`${screenshots}/${theme}-320-scrolled.png`,animations:'disabled'});
        await page.evaluate(()=>scrollTo(0,0));
      }
    }
  }
  await page.setViewportSize({width:1440,height:1100});
  const first=page.locator('.artifact-row-link').first();await first.focus();
  check(await first.evaluate(el=>el.matches(':focus-visible')),'Document keyboard focus is visible');
  await page.keyboard.press('Enter');await page.locator('#artifact-reading').waitFor();
  check(await page.locator('#artifact-reading').isVisible(),'Keyboard opens the full document');
  await page.getByRole('link',{name:'All artifacts',exact:true}).click();
  await page.locator('.artifact-row').first().waitFor();
  await page.getByRole('searchbox',{name:'Search artifacts'}).fill('yesterday');
  await page.waitForFunction(()=>document.querySelectorAll('.artifact-row').length===1);
  check((await page.locator('.artifact-row-preview').innerText()).includes('yesterday'),'Body search keeps its excerpt');
  await page.getByRole('searchbox',{name:'Search artifacts'}).fill('no-matching-document');
  await page.getByText('No matching artifacts',{exact:true}).waitFor();
  check(await page.getByRole('button',{name:'Clear search',exact:true}).isVisible(),'No-results state offers recovery');
  await page.getByRole('button',{name:'Clear search',exact:true}).click();
  await page.waitForFunction(()=>document.querySelectorAll('.artifact-row').length===6);
  await page.getByRole('button',{name:'Archive A calmer workspace',exact:true}).click();
  await page.waitForFunction(()=>document.querySelectorAll('.artifact-row').length===5);
  await page.getByRole('checkbox',{name:'Archived',exact:true}).check();
  await page.getByRole('button',{name:'Restore A calmer workspace',exact:true}).waitFor();
  check((await page.locator('.artifact-row-preview').innerText()).includes('Bring the work'),'Archived document preserves preview');
  await page.getByRole('button',{name:'Restore A calmer workspace',exact:true}).click();
  await page.getByText('No archived artifacts',{exact:true}).waitFor();
  await page.getByRole('button',{name:'Show active artifacts',exact:true}).click();
  await page.getByRole('button',{name:'Delete Untitled ideas permanently',exact:true}).click();
  await page.getByRole('dialog').waitFor();
  await page.getByRole('button',{name:'Cancel',exact:true}).click();
  check(await page.locator('.artifact-row').count()===6,'Cancel deletion keeps the card');
  // The next page must append cards with excerpts without resetting the first page.
  await page.route('**/api/action',async route=>{
    const payload=route.request().postDataJSON();if(payload.operation?.operation?.command!=='list')return route.continue();
    const offset=payload.operation.operation.offset||0;
    const artifacts=Array.from({length:offset?1:50},(_,i)=>({id:'page-'+(offset+i),title:'Document '+(offset+i),preview:'Bounded preview '+(offset+i),version:1,updated_at:Date.now(),archived:false}));
    await route.fulfill({json:{ok:true,artifacts,more:!offset}});
  });
  await page.reload();await page.getByRole('button',{name:'Show more',exact:true}).click();
  await page.waitForFunction(()=>document.querySelectorAll('.artifact-row').length===51);
  check(await page.locator('.artifact-row-preview').count()===51,'Pagination appends all previews');
  return {status:'passed',count:checks.length,checks};
})
