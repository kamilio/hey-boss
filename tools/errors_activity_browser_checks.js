// Run against a disposable real server; never writes to the live error inbox.
async page => {
  const assert = (value, message) => { if (!value) throw Error(message); };
  const project = 'named:Error activity QA', hour = 3600000, now = Date.now();
  await page.goto(new URL('/errors#'+new URLSearchParams({project}), page.url()).href);
  await page.evaluate(async ({project, now, hour}) => {
    const boot = await (await fetch('/api/bootstrap')).json();
    await HeyBossUI.post('/api/action', {project,operation:{action:'project_init',settings:{prs_enabled:false,worktree_enabled:false,if_version:0}}}, boot);
    await HeyBossUI.post('/api/errors', {action:'enable',project,enabled:true}, boot);
    const events = [
      ...Array.from({length:30}, (_,i) => ({source:'synthetic',event_id:'burst-'+i,timestamp:now-2*hour+i*1000,message:'Upload failed <diagnostic>',count:3})),
      {source:'synthetic',event_id:'quiet',timestamp:now-3*24*hour,message:'Older connection failure',count:2},
      {source:'synthetic',event_id:'active',timestamp:now-120000,message:'Current request failure',count:5},
    ];
    await HeyBossUI.post('/api/errors', {action:'ingest',project,events}, boot);
    await HeyBossUI.post('/api/errors', {action:'ingest',project,events}, boot);
  }, {project, now, hour});
  await page.getByRole('button', {name:'Refresh errors',exact:true}).click();
  const row = title => page.locator('#error-rows article').filter({has:page.getByRole('button',{name:title,exact:true})});
  const burst = row('Upload failed <diagnostic>'), quiet = row('Older connection failure');
  await burst.locator('.error-hourly-chart').waitFor();
  assert(await burst.locator('time').getAttribute('datetime') === new Date(now-2*hour+29000).toISOString(), 'Last event must use the newest occurrence time');
  assert(/ago/.test(await burst.locator('.error-last-event').innerText()), 'Last-event age should be visible');
  assert((await burst.locator('time').innerText()).length > 10, 'Exact timestamp should be visible without hover');
  const counts = await burst.locator('.error-hourly-chart rect').evaluateAll(bars => bars.map(b=>Number(b.dataset.count)));
  assert(counts.length === 24 && counts.reduce((a,b)=>a+b,0) === 90, 'Chart must include all 30 weighted events, once');
  assert(counts[23] === 0, 'Quiet latest hour should remain visible');
  assert(await quiet.locator('.error-hourly-chart rect[data-count="0"]').count() === 24, 'No recent events should render a flat chart');
  assert(await burst.locator('diagnostic').count() === 0, 'Diagnostic text was interpreted as HTML');
  assert(await burst.locator('.error-hourly-chart rect title').count() === 24, 'Every hourly bar needs its exact interval and count');
  await burst.locator('.error-hourly-chart').focus();
  assert(await burst.locator('.error-hourly-chart').evaluate(el=>document.activeElement===el), 'Chart must be keyboard accessible');
  await burst.getByRole('button', {name:'Upload failed <diagnostic>',exact:true}).click();
  await page.locator('#error-detail .error-last-event').waitFor();
  assert(await page.locator('#error-detail time').getAttribute('datetime') === new Date(now-2*hour+29000).toISOString(), 'Evidence must also show the last occurrence');
  await page.getByRole('button', {name:'Close error details',exact:true}).click();
  await page.getByRole('searchbox', {name:'Find an error',exact:true}).fill('Older');
  assert(await page.locator('#error-rows article').count() === 1, 'Search must preserve the quiet group');
  await page.getByRole('searchbox', {name:'Find an error',exact:true}).fill('');
  for (const colorScheme of ['light','dark']) {
    await page.emulateMedia({colorScheme});
    for (const width of [1280,768,390,320]) {
      await page.setViewportSize({width,height:900});
      assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth), `Horizontal overflow at ${width} (${colorScheme})`);
      const boxes = await burst.locator('.error-hourly-chart, .error-row-action').evaluateAll(elements=>elements.map(el=>{const r=el.getBoundingClientRect();return {left:r.left,right:r.right,top:r.top,bottom:r.bottom};}));
      assert(boxes.every(b=>b.left>=0&&b.right<=width), 'Activity and action must stay within the screen');
      assert(boxes[0].right<=boxes[1].left||boxes[0].bottom<=boxes[1].top||boxes[1].bottom<=boxes[0].top, 'Chart and row action overlap');
    }
  }
  await page.evaluate(async project => {
    const boot = await (await fetch('/api/bootstrap')).json();
    await HeyBossUI.post('/api/errors', {action:'ingest',project,events:[{source:'synthetic',event_id:'new-event',timestamp:Date.now()-1000,message:'Upload failed <diagnostic>',count:4}]}, boot);
  }, project);
  await page.getByRole('button', {name:'Refresh errors',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('.error-hourly-chart rect:last-child')?.dataset.count==='4');
  assert((await burst.locator('.error-count').innerText()).includes('94'), 'Refresh must update chart and count together');
  return {weightedCounts:true,replay:true,quietHours:true,lastEvent:true,detail:true,refresh:true,search:true,keyboard:true,responsive:true,themes:true};
}
