// Run with playwright-cli run-code --filename against an isolated issue web server.
async page => {
  const checks=[], errors=[];
  const check=(value,label)=>{if(!value)throw Error(label);checks.push(label);};
  page.on('pageerror',error=>errors.push(error.message));
  page.setDefaultTimeout(20000);
  const {base,paired}=await page.evaluate(()=>({base:location.origin,paired:location.pathname==='/issues'}));
  const path=paired?'/issues':'/';
  // Use current source assets with an older fixture binary as well.
  if(!paired){
    for(const name of ['app.js','inbox.js','components.js','pagination.js','pagination.css'])
      await page.route(base+'/'+name,route=>route.fulfill({path:'src/issues/web/'+name}));
    await page.route(base+'/',async route=>{
      const response=await route.fetch();let body=await response.text();
      if(!body.includes('/pagination.js'))body=body.replace('</head>','<script src="/pagination.js" defer></script><link rel="stylesheet" href="/pagination.css"></head>');
      if(!body.includes('id="issue-pagination-top"'))body=body.replace('<div id="issue-list"','<div id="issue-pagination-top"></div><div id="issue-pagination-bottom"></div><div id="issue-list"');
      await route.fulfill({response,body});
    });
  }
  await page.goto(base+path);
  await page.waitForFunction(()=>model.csrf && model.signature);
  const sample=await page.evaluate(async()=>{
    const created=await api({action:'create',title:'Pagination fixture',body:'A preserved full detail.',labels:[],draft:true});
    return api({action:'view',number:created.issue.number});
  });
  let count=2005;
  const issues=()=>Array.from({length:count},(_,index)=>({...sample.issue,number:index+1,title:`Pagination task ${String(index+1).padStart(4,'0')}`,labels:index===2004?['last-page']:[],draft:false,comment_count:0}));
  await page.route('**/api/action',async route=>{
    const op=route.request().postDataJSON().operation;
    if(op.action==='list'){
      let rows=issues().map(issue=>({...issue,state:op.state,closed_at:op.state==='closed'?Date.now():null}));
      if(op.search)rows=rows.filter(issue=>issue.title.includes(op.search));
      if(op.labels?.length)rows=rows.filter(issue=>op.labels.every(label=>issue.labels.includes(label)));
      return route.fulfill({json:{ok:true,project:sample.project,order_version:1,issues:rows,next_offset:null}});
    }
    if(op.action==='view')return route.fulfill({json:{...sample,issue:{...issues()[op.number-1],state:'closed'},comments:[]}});
    return route.continue();
  });
  let notices=Array.from({length:4010},(_,i)=>({taskID:'notice-'+i,title:`Notification ${String(i%2005+1).padStart(4,'0')}`,summary:'Build completed and verified across the fleet.',project:i%2005===2004?'Other project':'Workspace',kind:'alert',severity:'success',status:i<2005?'pending':'resolved',sourceHost:'MacBook',createdAt:Date.now()/1000-i,completedAt:i<2005?null:Date.now()/1000-i}));
  await page.route('**/api/inbox',route=>{
    const op=route.request().postDataJSON();
    const task=notices.find(task=>task.taskID===op.task_id);
    return route.fulfill({json:op.action==='view'||op.action==='read'?{ok:true,task:{...task,body_html:'<p>Full notice body.</p>'}}:{ok:true,unread:notices.filter(t=>t.status==='pending').length,tasks:op.action==='related'?[]:notices}});
  });
  const top=()=>page.locator('#issue-pagination-top .list-pagination');
  const go=async hash=>{await page.goto(base+path+hash);await page.waitForFunction(()=>model.signature && !model.route.issue);};
  await go('#state=closed');
  await page.waitForFunction(()=>document.querySelectorAll('#issue-list .issue-row').length===1000);
  check((await top().innerText()).includes('1–1,000'),'First-page range and formatted total');
  check(await top().locator('[aria-disabled=true]').count()===1,'Previous disabled on first page');
  await top().getByRole('link',{name:'Next page of issues'}).focus();
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>model.route.page===2);
  check(await page.locator('.issue-title').first().innerText()==='Pagination task 1001','Keyboard Next starts at item 1,001');
  check(await page.evaluate(()=>document.activeElement.classList.contains('list-pagination')),'Page navigation restores visible keyboard focus');
  await top().getByRole('link',{name:'Previous page of issues'}).focus();
  await page.evaluate(()=>refresh(false));
  check(await page.evaluate(()=>document.activeElement.getAttribute('aria-label'))==='Previous page of issues','Background refresh preserves pager keyboard focus');
  await page.locator('.issue-title').first().click();
  await page.locator('[data-back]').click();
  await page.waitForFunction(()=>model.route.page===2 && !model.route.issue && document.querySelector('.issue-title')?.textContent==='Pagination task 1001');
  check(true,'Detail and Back preserve page');
  await page.reload();
  await page.waitForFunction(()=>document.querySelector('.issue-title')?.textContent==='Pagination task 1001');
  check(true,'Reload preserves page');
  await top().getByRole('link',{name:'Next page of issues'}).click();
  await page.waitForFunction(()=>document.querySelectorAll('#issue-list .issue-row').length===5);
  check(await page.locator('.issue-title').last().innerText()==='Pagination task 2005','Last page contains every remaining item');
  await page.goBack();
  await page.waitForFunction(()=>model.route.page===2 && document.querySelector('.issue-title')?.textContent==='Pagination task 1001');
  check(true,'Browser Back preserves page');
  await page.locator('#issue-search').fill('2005');
  await page.waitForFunction(()=>document.querySelectorAll('.issue-row').length===1);
  check(await top().count()===0,'Search covers later pages and resets pagination');
  await go('#state=closed&page=999');
  await page.waitForFunction(()=>model.route.page===3);
  count=1000;
  await page.evaluate(()=>refresh(false));
  check(await page.evaluate(()=>model.route.page)===1 && await top().count()===0,'Refresh clamps page when results shrink to 1,000');
  count=2005;
  await go('#state=open&page=2');
  await page.waitForFunction(()=>document.querySelector('.issue-title')?.textContent==='Pagination task 1001');
  await page.evaluate(async()=>{
    model.creation={project:model.project.id,host:model.route.host,number:2005,reveal:true,filtersChecked:true};
    renderList(await api(listOperation()));
  });
  check(await page.evaluate(()=>model.route.page)===3 && await page.locator('[data-issue-number="2005"].issue-created').count()===1,'New issue is revealed on its page');
  await page.locator('[data-state=closed]').click();
  await page.waitForFunction(()=>model.route.page===1 && model.route.state==='closed');
  check(true,'Changing issue state resets to first page');
  for(const theme of ['light','dark']) for(const width of [1440,768,390,320]){
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    await page.setViewportSize({width,height:900});
    await page.evaluate(()=>scrollTo(0,0));
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`Issues fit ${theme}/${width}`);
    check(await top().locator('a').evaluateAll(links=>links.every(link=>link.getBoundingClientRect().height>=44)),`Touch targets ${theme}/${width}`);
    await page.screenshot({path:`/tmp/hb-list-pagination-${paired?'paired':'desktop'}-${theme}-${width}.png`});
  }
  await page.goto(base+path+'#view=inbox&inbox_page=2');
  await page.waitForFunction(()=>document.querySelectorAll('.notice-row').length===1000);
  check(await page.locator('.notice-title').first().innerText()==='Notification 1001','Inbox second page');
  await page.locator('.notice-title').first().click();
  await page.locator('#inbox-view .back-link').click();
  await page.waitForFunction(()=>document.querySelector('.notice-title')?.textContent==='Notification 1001');
  check(true,'Notice detail preserves Inbox page');
  await page.locator('[data-inbox-state=archive]').click();
  await page.waitForFunction(()=>model.route.inbox_page===1 && model.route.inbox_state==='archive');
  await page.locator('#inbox-view .list-pagination').first().getByRole('link',{name:'Next page of notices'}).click();
  await page.waitForFunction(()=>model.route.inbox_page===2);
  await page.locator('#inbox-search').fill('2005');
  await page.waitForFunction(()=>document.querySelectorAll('.notice-row').length===1);
  check(await page.locator('.notice-title').innerText()==='Notification 2005','Activity search reaches final page');
  await page.locator('#inbox-search').fill('');
  await page.waitForFunction(()=>document.querySelectorAll('.notice-row').length===1000);
  await page.locator('#inbox-project-filter').selectOption('Other project');
  await page.waitForFunction(()=>document.querySelectorAll('.notice-row').length===1);
  check(true,'Project filter covers all pages');
  await page.locator('#inbox-project-filter').selectOption('');
  await page.waitForFunction(()=>document.querySelectorAll('.notice-row').length===1000);
  for(const theme of ['light','dark']){
    await page.emulateMedia({colorScheme:theme});
    await page.evaluate(()=>scrollTo(0,0));
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`Inbox fits ${theme}/320`);
    await page.screenshot({path:`/tmp/hb-list-pagination-inbox-${paired?'paired':'desktop'}-${theme}.png`});
  }
  check(errors.length===0,'No browser runtime errors: '+errors.join('; '));
  return {checks:checks.length,passed:checks};
}
