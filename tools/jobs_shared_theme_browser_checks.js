// Run against the isolated native and paired Jobs fixture.
async page => {
 const origin=new URL(page.url()).origin,paired=origin.endsWith('52049');
 if(!['http://127.0.0.1:59649','http://127.0.0.1:52049'].includes(origin))throw Error('Isolated fixture required');
 const project='named:Jobs UI QA',checks=[];
 const check=(value,name)=>{if(!value)throw Error(name);checks.push(name);};
 const styles=()=>{
  const selectors=['body','.app-header','.app-navigation','#project-trigger','#project-menu','.navigation-more-links','.button.primary'];
  const properties=['backgroundColor','backgroundImage','color','borderColor','borderRadius','fontFamily','fontSize','fontWeight','boxShadow'];
  return Object.fromEntries(selectors.map(selector=>{const node=document.querySelector(selector);if(!node)return [selector,null];const style=getComputedStyle(node);return [selector,Object.fromEntries(properties.map(property=>[property,style[property]]))];}));
 };
 for(const width of [1440,390])for(const colorScheme of ['light','dark']){
  await page.setViewportSize({width,height:1000});await page.emulateMedia({colorScheme});
  let expected;
  for(const path of [paired?'/issues':'/','/jobs']){
   await page.goto(origin+path+'#project='+encodeURIComponent(project));
   await page.waitForFunction(()=>document.querySelector('#project-name')?.textContent==='Jobs UI QA');
   await page.locator('#project-trigger').click();await page.locator('#project-menu').waitFor({state:'visible'});
   const actual=await page.evaluate(styles);
   if(expected)check(JSON.stringify(actual)===JSON.stringify(expected),`${width} ${colorScheme}: Jobs shares issue colors, typography, menus and primary button`);else expected=actual;
   const box=await page.locator('#project-menu').boundingBox();check(box.x>=0&&box.x+box.width<=width,`${path} menu fits ${width}`);
   await page.keyboard.press('Escape');
   check(await page.locator('#project-trigger').evaluate(e=>e===document.activeElement),`${path} menu restores focus`);
   check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${path} ${width} ${colorScheme}: no overflow`);
   if(path==='/jobs')await page.screenshot({path:`/tmp/hb-calendar-qa/${paired?'paired':'desktop'}-shared-theme-${width}-${colorScheme}.png`,fullPage:true});
  }
 }
 return {checks:checks.length};
}
