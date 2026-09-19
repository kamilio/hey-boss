async page => {
 const checks=[],check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name)},errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.emulateMedia({colorScheme:'light'});await page.goto('http://127.0.0.1:4782/');await page.waitForFunction(()=>model.csrf&&model.project);
 const project=await page.evaluate(async()=>{const value=await api({action:'create',title:'Automatic appearance QA',body:'> [!WARNING]\n> Verify warning colors.\n\n```rust\nlet n = 42;\n```',labels:[]},'Auto appearance QA '+Date.now());localStorage.setItem('hey-boss-issues-theme',JSON.stringify('dark'));return value.project.id});
 await page.goto('http://127.0.0.1:4782/#project='+encodeURIComponent(project));await page.waitForSelector('.issue-row');
 const appearance=()=>page.evaluate(()=>({scheme:getComputedStyle(document.documentElement).colorScheme,bg:getComputedStyle(document.documentElement).getPropertyValue('--bg').trim()}));
 check((await appearance()).scheme==='light','System light ignores old saved dark preference');
 check(await page.locator('#theme,[aria-label*="Switch color theme"]').count()===0,'Theme switcher removed');
 await page.emulateMedia({colorScheme:'dark'});await page.waitForFunction(()=>getComputedStyle(document.documentElement).colorScheme==='dark');check((await appearance()).bg==='#11151d','Live system dark change updates colors');
 await page.screenshot({path:'output/playwright/issues-auto-dark.png'});
 await page.locator('#new-issue').click();await page.locator('#editor-subject').fill('Keep this draft');await page.locator('#editor-body').fill('Draft markdown');
 await page.emulateMedia({colorScheme:'light'});await page.waitForFunction(()=>getComputedStyle(document.documentElement).colorScheme==='light');
 check(await page.locator('#editor-subject').inputValue()==='Keep this draft'&&await page.locator('#editor-body').inputValue()==='Draft markdown','Live appearance change preserves editor draft');await page.keyboard.press('Escape');
 await page.locator('.issue-title').click();await page.waitForSelector('.markdown-alert-warning');
 const warning=()=>page.locator('.markdown-alert-warning').evaluate(el=>getComputedStyle(el).getPropertyValue('--alert').trim());
 check(await warning()==='#936415','Light Markdown warning color');await page.emulateMedia({colorScheme:'dark'});await page.waitForFunction(()=>getComputedStyle(document.documentElement).colorScheme==='dark');check(await warning()==='#e3b864','Dark Markdown warning color');
 await page.setViewportSize({width:390,height:844});check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Mobile automatic appearance fits viewport');
 await page.reload();await page.waitForSelector('.markdown-alert-warning');check((await appearance()).scheme==='dark','Dark system setting persists across reload without override');
 await page.emulateMedia({colorScheme:'light'});await page.waitForFunction(()=>getComputedStyle(document.documentElement).colorScheme==='light');await page.screenshot({path:'output/playwright/issues-auto-light.png'});
 // CSS must choose the system appearance before any application script runs.
 const context=await page.context().browser().newContext({javaScriptEnabled:false,colorScheme:'dark'});
 try {const first=await context.newPage();await first.goto('http://127.0.0.1:4782/');check(await first.evaluate(()=>getComputedStyle(document.documentElement).colorScheme)==='dark','Dark appearance works without JavaScript on first paint');} finally {await context.close()}
 check(errors.length===0,'No browser errors');return {passed:checks.length,checks,project};
}
