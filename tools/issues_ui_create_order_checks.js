async page => {
 await page.goto('http://127.0.0.1:4782/');await page.waitForFunction(()=>model.csrf&&model.project);
 const project=await page.evaluate(async()=>{const name='UI creation order QA '+Date.now();let project;for(const title of ['Existing one','Existing two'])project=(await api({action:'create',title,body:'',labels:[]},name)).project.id;await api({action:'move',number:2,before:1},project);return project});
 await page.goto('http://127.0.0.1:4782/#project='+encodeURIComponent(project));await page.waitForSelector('.issue-order-handle');
 const checks=[];
 const expect=async(order,name)=>{await page.waitForFunction(order=>JSON.stringify([...document.querySelectorAll('.issue-row')].map(row=>Number(row.dataset.issueNumber)))===JSON.stringify(order),order);checks.push(name)};
 await expect([2,1],'Existing manual order');
 for(const [number,title] of [[3,'New from desktop'],[4,'New from mobile']]){
  if(number===4)await page.setViewportSize({width:390,height:844});
  await page.locator('#new-issue').click();await page.locator('#editor-subject').fill(title);await page.locator('#editor-body').fill('# Created through the form');await page.locator('#editor-submit').click();
  await page.waitForFunction(number=>model.route.issue===null&&document.querySelector(`[data-issue-number="${number}"].issue-created`),number);checks.push('Creation stays on list and highlights issue '+number);
  await expect(number===3?[3,2,1]:[4,3,2,1],'Form-created issue '+number+' goes to top');
 }
 await page.evaluate(async()=>{await api({action:'create',title:'CLI default append',body:'',labels:[]},model.project.id)});await page.reload();await expect([4,3,2,1,5],'Default creation appends and order persists');
 const records=await page.evaluate(async()=>api({action:'list',state:'open',mine:false,unassigned:false,labels:[],search:null,limit:50,offset:0},model.project.id));
 if(new Set(records.issues.map(i=>i.sort_order)).size!==5)throw Error('Duplicate positions');checks.push('Unique queue positions');
 await page.evaluate(async()=>{for(let n=6;n<=125;n++)await api({action:'create',title:'Bulk issue '+n,body:'',labels:n%2?[]:['ready']},model.project.id)});
 await page.goto('http://127.0.0.1:4782/#project='+encodeURIComponent(project)+'&offset=100');
 await page.waitForFunction(()=>document.querySelectorAll('.issue-row').length===125);checks.push('All 125 issues render despite a legacy page offset');
 if(await page.locator('#previous-page,#next-page').count())throw Error('Pagination remains');checks.push('No pagination controls');
 if(await page.locator('#list-summary').innerText()!=='125 issues')throw Error('Incorrect total');checks.push('Complete list total');
 await page.locator('#label-filter').selectOption('ready');await page.waitForFunction(()=>document.querySelectorAll('.issue-row').length===60);checks.push('All filtered issues render');
 await page.locator('#label-filter').selectOption('');await page.waitForFunction(()=>document.querySelectorAll('.issue-row').length===125);
 await page.locator('[data-move-issue="125"]').focus();await page.keyboard.press('ArrowUp');await page.waitForFunction(()=>!model.orderSaving&&model.issues[123].number===125);checks.push('Last issue can reorder on the same page');
 await page.reload();await page.waitForFunction(()=>model.issues.length===125&&model.issues[123].number===125);checks.push('Full list and bottom reorder persist');
 return {passed:checks.length,checks,project};
}
