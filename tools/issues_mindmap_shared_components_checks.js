// Run through playwright-cli against an isolated issue web server on port 4782.
async page => {
 page.setDefaultTimeout(90000); page.setDefaultNavigationTimeout(90000);
 await page.unroute('**/components.css');
 await page.setViewportSize({width:1440,height:1000});
 const base='http://127.0.0.1:4782'; const checks=[];
 const check=(value,name)=>{if(!value)throw new Error(name);checks.push(name);};
 const errors=[];page.on('pageerror',error=>errors.push(error.message));
 const boot=await (await page.request.get(base+'/api/bootstrap')).json();
 const action=async(project,operation)=>{
  const r=await page.request.post(base+'/api/action',{headers:{'X-Hey-Boss-CSRF':boot.csrf},data:{project,operation,request_id:null}});
  if(!r.ok())throw new Error(await r.text()); return r.json();
 };
 for(const project of ['named:Alpha project','named:Beta project']) await action(project,{action:'list',state:'open',mine:false,unassigned:false,labels:[],search:null,limit:50,offset:0});
 await action('named:Hidden QA',{action:'list',state:'open',mine:false,unassigned:false,labels:[],search:null,limit:50,offset:0});
 await action('named:Hidden QA',{action:'hide_project'});
 let issueShell;
 for(const route of ['/', '/mm']) {
  await page.goto(base+route+'#project='+encodeURIComponent('named:Alpha project'));
  await page.waitForFunction(()=>document.querySelector('#project-name').textContent==='Alpha project');
  check(await page.locator('.app-header').count()===1,route+' uses one shared header');
  const shell=await page.locator('.app-header').evaluate(el=>({height:el.offsetHeight,background:getComputedStyle(el).backgroundColor}));
  if(route==='/')issueShell=shell;else check(JSON.stringify(shell)===JSON.stringify(issueShell),'Headers share size and theme');
  await page.locator('#project-trigger').click();
  check(await page.locator('#project-search').evaluate(el=>el===document.activeElement),route+' picker focuses search');
  await page.locator('#project-search').fill('BETA PROJECT');
  check(await page.locator('.project-activity time').count()===1,route+' uses shared activity formatting');
  check(await page.locator('.project-option').count()===1,route+' filters projects');
  await page.keyboard.press('ArrowDown');
  check(await page.locator('.project-option').evaluate(el=>el===document.activeElement),route+' arrow key reaches filtered option');
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>document.querySelector('#project-name').textContent==='Beta project');
  check(await page.evaluate(()=>new URLSearchParams(location.hash.slice(1)).get('project'))==='named:Beta project',route+' selects project through URL');
  check(await page.locator('#project-menu').isHidden(),route+' selection closes picker');
  check(await page.locator('#nav-mindmaps').evaluate(el=>new URLSearchParams(new URL(el.href).hash.slice(1)).get('project'))==='named:Beta project',route+' navigation preserves project context');
  if(route==='/mm')check(await page.locator('.map-project-title').textContent()==='Beta project','Project switch loads correct map');
  await page.locator('#project-trigger').click();await page.locator('#project-search').fill('no-match-123');
  check(await page.locator('.menu-empty').textContent()==='No matching projects.',route+' empty search state');
  await page.locator('#project-search').fill('');await page.locator('#toggle-hidden-projects').click();
  check(await page.locator('.project-option strong').filter({hasText:'Hidden QA'}).count()===1,route+' hidden projects remain accessible');
  if(route==='/mm')check(await page.locator('[data-project-visibility]').count()===0,'Mindmap picker remains read-only');
  await page.keyboard.press('Escape');
  check(await page.locator('#project-trigger').evaluate(el=>el===document.activeElement),route+' Escape restores trigger focus');
  await page.locator('#project-trigger').click();await page.mouse.click(4,400);
  check(await page.locator('#project-menu').isHidden(),route+' outside click closes picker');
  await page.reload();await page.waitForFunction(()=>document.querySelector('#project-name').textContent==='Beta project');
  check(true,route+' project survives reload');
 }
 for(const width of [1440,390,320]) {
  await page.setViewportSize({width,height:900});
  await page.locator('#map-fit').click();
  for(const scheme of ['light','dark']) {
   await page.emulateMedia({colorScheme:scheme});
   await page.locator('#project-trigger').click();
   const box=await page.locator('#project-menu').boundingBox();
   check(box.x>=0 && box.x+box.width<=width,width+' '+scheme+' picker fits viewport');
   check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),width+' '+scheme+' page has no horizontal overflow');
   await page.screenshot({path:`output/playwright/issue11-mindmaps-${width}-${scheme}.png`});
   await page.keyboard.press('Escape');
  }
 }
 await page.setViewportSize({width:1440,height:1000});await page.emulateMedia({colorScheme:'light'});
 await page.locator('#view-outline').click();
 check(await page.locator('#outline').isVisible(),'Outline still works');
 await page.locator('#view-map').click();
 check(await page.locator('#mindmap').isVisible(),'Map still works');
 check(!errors.length,'No JavaScript errors: '+errors.join(';'));
 return {passed:checks.length,checks};
}
