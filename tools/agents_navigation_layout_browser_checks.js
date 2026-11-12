// Run with playwright-cli run-code against the agents-navigation web-fixture scenario.
async page => {
  const checks=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  const visit=async path=>{await page.goto('http://127.0.0.1:59643'+path);await page.waitForFunction(()=>document.querySelector('#project-name').textContent!=='Projects');};
  for(const width of [1280,390,320]) {
    await page.setViewportSize({width,height:844});
    for(const [name,path] of [['Workers','/agents'],['Legacy Workers','/agents#view=conversations'],['Session','/agents/session']]) {
      await visit(path);
      const layout=await page.locator('.app-navigation').evaluate(nav=>{
        const links=[...nav.querySelectorAll('.navigation-inner > a')];
        return {
          fits:document.documentElement.scrollWidth<=innerWidth,
          reachable:links.filter(link=>!link.hidden).every(link=>{link.scrollIntoView({block:'nearest',inline:'nearest'});const r=link.getBoundingClientRect();return r.left>=0 && r.right<=innerWidth+1;}),
          readable:links.every(link=>{
            const box=link.getBoundingClientRect();
            return [...link.childNodes].filter(node=>node.nodeType===Node.TEXT_NODE&&node.textContent.trim()).every(node=>{
              const range=document.createRange();range.selectNodeContents(node);
              const text=range.getBoundingClientRect();
              return text.left>=box.left&&text.right<=box.right;
            });
          }),
        };
      });
      check(layout.fits,width+'px '+name+' fits');
      check(layout.readable,width+'px '+name+' navigation labels fit their links');
      if(width<560) {
        check(layout.reachable,width+'px '+name+' project links can be brought fully into view');
        await page.locator('#navigation-more').focus();
        await page.keyboard.press('Enter');
        await page.keyboard.press('Tab');
        check(await page.locator('#nav-workers').evaluate(link=>{
          const box=link.getBoundingClientRect();
          return document.activeElement===link&&box.left>=0&&box.right<=innerWidth;
        }),width+'px '+name+' Workers link is reachable by keyboard');
        await page.keyboard.press('Escape');
      }
    }
  }
  return {passed:checks.length,checks};
}
