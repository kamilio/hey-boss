// Run with playwright-cli run-code against serve_agents_navigation_fixture.mjs.
async page => {
  const checks=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  const visit=async path=>{await page.goto('http://127.0.0.1:59643'+path);await page.waitForFunction(()=>document.querySelector('#project-name').textContent!=='Projects');};
  for(const width of [1280,390,320]) {
    await page.setViewportSize({width,height:844});
    for(const [name,path] of [['Workers','/agents'],['Conversations','/agents#view=conversations'],['Session','/agents/session']]) {
      await visit(path);
      const layout=await page.locator('.app-navigation').evaluate(nav=>{
        const links=[...nav.querySelectorAll('.navigation-inner > a')];
        return {
          fits:document.documentElement.scrollWidth<=innerWidth,
          scrolls:nav.scrollWidth>nav.clientWidth,
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
        check(layout.scrolls,width+'px '+name+' navigation scrolls');
        await page.locator('#nav-skills').focus();
        await page.keyboard.press('Tab');
        check(await page.locator('#nav-admin').evaluate(link=>{
          const box=link.getBoundingClientRect();
          return document.activeElement===link&&box.left>=0&&box.right<=innerWidth;
        }),width+'px '+name+' last link is reachable by keyboard');
      }
    }
  }
  return {passed:checks.length,checks};
}
