// Run through playwright-cli against the isolated fixtures on port 4782.
async (page) => {
  const base = "http://127.0.0.1:4782/";
  await page.goto(base); await page.reload();
  await page.locator("#new-issue").waitFor();
  const bootstrap = await (await page.request.get(base + "api/bootstrap")).json();
  const suffix = Date.now();
  const alpha = `named:Project QA Alpha ${suffix}`;
  const beta = `named:Project QA Beta ${suffix}`;
  const alphaName = alpha.slice(6), betaName = beta.slice(6);
  const checks = [];
  const check = (value, name) => { if (!value) throw new Error(name); checks.push(name); };
  const action = async (project, operation) => {
    const response = await page.request.post(base + "api/action", {
      headers: { "X-Hey-Boss-CSRF": bootstrap.csrf }, data: { project, operation, request_id: null }
    });
    if (!response.ok()) throw new Error(await response.text());
    return response.json();
  };
  const list = {action:"list",state:"open",mine:false,unassigned:false,labels:[],search:null,limit:50,offset:0};
  await action(alpha, list); await action(beta, list);
  await page.evaluate(project => location.hash = "project=" + encodeURIComponent(project), beta);
  await page.getByRole("heading", {name:"A clear place to start",exact:true}).waitFor();
  await page.locator("#project-trigger").click();
  await page.locator("#project-search").fill("Project QA");
  await page.getByRole("button", {name:"Hide " + alphaName,exact:true}).waitFor();
  check(await page.locator(".project-option strong").filter({hasText:alphaName}).count() === 1, "Projects appear before any issue is created");
  await action(alpha, {action:"create",title:"Keep project data",body:"## Retained Markdown",labels:["retained"]});
  await page.keyboard.press("Escape");
  await page.getByRole("button", {name:"Refresh issues",exact:true}).click();
  await page.locator("#project-trigger").click();
  await page.locator("#project-search").fill("Project QA");
  await page.waitForFunction(name => document.querySelector(".project-option strong")?.textContent === name,alphaName);
  check(true,"Issue activity moves a project to the top of the switcher");
  const option = page.locator(".project-option").filter({has:page.locator("strong",{hasText:alphaName})});
  await option.focus();
  await page.waitForTimeout(5500); // Deliberately span the five-second refresh cycle.
  check(await page.evaluate(() => document.activeElement?.classList.contains("project-option")),"Background refresh preserves keyboard focus in the switcher");
  await page.getByRole("button", {name:"Hide " + alphaName,exact:true}).click();
  await page.getByRole("button", {name:"Hide " + alphaName,exact:true}).waitFor({state:"detached"});
  check(await page.locator(".project-option strong").filter({hasText:alphaName}).count()===0,"Hide removes the project from active projects");
  await page.locator("#toggle-hidden-projects").click();
  await page.getByRole("button", {name:"Restore " + alphaName,exact:true}).waitFor();
  await page.locator(".project-option").filter({has:page.locator("strong",{hasText:alphaName})}).click();
  await page.locator("#hidden-project-banner").waitFor();
  await page.getByRole("link",{name:"Keep project data",exact:true}).click();
  await page.getByRole("heading",{name:"Retained Markdown",exact:true}).waitFor();
  check(true,"A hidden project's issue and Markdown remain accessible");
  await action(alpha,{action:"comment",number:1,body:"Activity while hidden"});
  const hidden = await action(alpha,{action:"projects",include_hidden:true});
  check(Boolean(hidden.projects.find(p=>p.id===alpha).hidden_at),"New comments do not restore hidden projects");
  await page.getByRole("button",{name:"Restore project",exact:true}).click();
  await page.locator("#hidden-project-banner").waitFor({state:"hidden"});
  await page.locator("#project-trigger").click();
  await page.locator("#toggle-hidden-projects").click();
  await page.locator("#project-search").fill("Project QA");
  await page.getByRole("button",{name:"Hide " + alphaName,exact:true}).waitFor();
  check(true,"Restore returns the project to the active list");
  await page.setViewportSize({width:390,height:844});
  const menu = await page.locator("#project-menu").boundingBox();
  check(menu.x>=0 && menu.x+menu.width<=390,"Project activity and hide controls fit mobile screens");
  await page.screenshot({path:"output/playwright/issues-project-management-mobile.png"});
  await page.setViewportSize({width:1440,height:1000});
  await page.screenshot({path:"output/playwright/issues-project-management-desktop.png"});
  await page.keyboard.press("Escape");
  await page.reload();
  const restored = await action(alpha,{action:"view",number:1});
  check(restored.comments.some(c=>c.body==="Activity while hidden"),"Restore preserves activity recorded while hidden");
  return {passed:checks.length,checks};
}
