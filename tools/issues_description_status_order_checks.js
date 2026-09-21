async page => {
 const checks=[];
 const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 const fixture='http://127.0.0.1:52070';
 const {code}=await (await page.request.get(fixture+'/fixture/pairing')).json();
 check((await page.request.post(fixture+'/api/pair',{data:{code}})).ok(),'Fixture device paired');
 for(const mode of ['paired','desktop'])for(const theme of ['light','dark'])for(const width of [1440,768,390,320]){
  await page.setViewportSize({width,height:1000});
  await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
  const base=mode==='desktop'?'http://127.0.0.1:4796/':fixture+'/project-resource';
  for(const number of [1,4,5]){
   const label=`${mode}/${theme}/${width}/issue-${number}`;
   await page.goto(base+'?qa='+Date.now()+'#project=named%3AProgress%20Studio&issue='+number,{waitUntil:'commit'});
   await page.locator('.issue-progress-card').waitFor();
   const mainSelector=mode==='desktop'?'.detail-main':'#resource-content';
   const order=await page.locator(mainSelector).evaluate(main=>{
    const description=main.querySelector(':scope > article');
    const progress=main.querySelector(':scope > .issue-progress-card');
    return (!main.matches('.detail-main')||main.firstElementChild===description)&&description.nextElementSibling===progress&&
     description.getBoundingClientRect().bottom+16<=progress.getBoundingClientRect().top;
   });
   check(order,label+': description comes first, with space before status');
   check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),label+': no horizontal overflow');
   if(number===1){
    await page.locator('.progress-history summary').focus();await page.keyboard.press('Enter');
    await page.locator('.progress-history-list li').nth(19).waitFor();
    check(await page.locator(mainSelector).evaluate(main=>main.querySelector(':scope > article').nextElementSibling.matches('.issue-progress-card')),label+': expanded history stays after description');
    await page.locator('.progress-history summary').click();
    await page.screenshot({path:`output/playwright/issue89/${mode}-${theme}-${width}.png`,fullPage:true});
   }else{
    check(number===4?await page.locator('.progress-empty').count()===1:(await page.locator('.progress-comment').textContent()).length===500,label+': empty and long status stay intact');
   }
  }
 }
 return {checks:checks.length};
}
