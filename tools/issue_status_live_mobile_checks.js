async page => {
 const errors=[],checks=[];const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 await page.unrouteAll({behavior:'wait'});page.on('pageerror',e=>errors.push(e.message));
 const ids=new Set();let polls=0;
 const status={id:'phone-live-first',author:'human:qa',level:'green',comment:'Checking the phone layout now.',created_at:Date.now()};
 await page.route('**/api/artifact-requests',async route=>{
  const response=await route.fetch(),body=await response.json();
  if(route.request().postDataJSON()?.operation.action==='status_view'){ids.add(body.request.id);polls++;}
  await route.fulfill({response,json:body});
 });
 await page.route('**/api/artifact-requests/*',async route=>{
  const response=await route.fetch(),body=await response.json();
  if(ids.has(body.request?.id)&&body.request.status==='done'){
   body.request.result.status={...status};body.request.result.assignee='human:qa';
  }
  await route.fulfill({response,json:body});
 });
 await page.goto(`http://127.0.0.1:52070/project-resource?live=${Date.now()}#project=named%3AProgress%20Studio&issue=4`,{waitUntil:'domcontentloaded'});
 await page.locator('.progress-empty').waitFor();
 await page.getByText(status.comment,{exact:true}).waitFor({timeout:35000});
 check(polls>0,'Visible paired viewer polls the small status endpoint');
 check(await page.locator('.progress-history').count()===1,'First phone update adds collapsed history');
 check(await page.locator('.progress-history').getAttribute('open')===null,'Phone polling does not open history');
 check(await page.locator('.progress-badge').textContent()==='On track','First phone update displays green');
 status.id='phone-live-second';status.level='red';status.comment='The connection needs attention.';
 await page.getByText(status.comment,{exact:true}).waitFor({timeout:35000});
 check(await page.locator('.progress-badge').textContent()==='In trouble','Later phone update changes color and message');
 check(await page.locator('.progress-history textarea,.progress-current button').count()===0,'Phone remains read-only after live updates');
 await page.unrouteAll({behavior:'wait'});
 check(errors.length===0,`No live phone errors: ${errors}`);
 return {checks:checks.length,polls,errors};
}
