// Following a moved issue with the keyboard must keep its handle visible.
async page => {
  page.removeAllListeners('dialog');page.on('dialog',d=>d.accept().catch(()=>{}));
  const origin=await page.evaluate(()=>location.origin),checks=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name)};
  await page.goto(origin+'/?keyboard-order='+Date.now());await page.waitForFunction(()=>model.csrf&&model.project);
  const project=await page.evaluate(async()=>{const name='Keyboard order QA '+Date.now();for(let n=1;n<=25;n++)await api({action:'create',title:'Follow issue '+n,body:'',labels:[]},name);return 'named:'+name});
  await page.goto(origin+'/?keyboard-order='+Date.now()+'#project='+encodeURIComponent(project));
  await page.waitForFunction(project=>model.project?.id===project&&model.signature&&model.issues.length===25,project);
  await page.setViewportSize({width:320,height:700});const handle=page.locator('[data-move-issue="22"]');await handle.focus();
  for(let move=1;move<=15;move++){
    await page.keyboard.press('ArrowUp');await page.waitForFunction(index=>!model.orderSaving&&model.issues[index]?.number===22,21-move);
    await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
    check(await handle.evaluate(el=>{const r=el.getBoundingClientRect();return el===document.activeElement&&r.top>=0&&r.bottom<=innerHeight&&el.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2))}),'Moved issue stays focused and visible after step '+move);
  }
  const canonical=await page.evaluate(async()=>(await api(listOperation())).issues.map(i=>i.number));
  check(canonical[6]===22,'Canonical queue follows keyboard moves');
  check(await page.locator('.issue-row').count()===25,'All issues remain on the same page');
  return {passed:checks.length,checks};
}
