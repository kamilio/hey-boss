// Run with playwright-cli run-code against an isolated issue server on 4795.
async page => {
  page.setDefaultTimeout(60000);
  page.removeAllListeners("dialog");
  page.on("dialog", dialog => dialog.type() === "beforeunload" ? dialog.accept() : dialog.dismiss());
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on("pageerror", error => errors.push(error.message));
  const base = "http://127.0.0.1:4795/#project=named%3ABlocked%20QA";
  await page.goto("about:blank");
  await page.goto(base + "&state=blocked");
  await page.locator('.issue-row').first().waitFor();
  check(await page.getByRole("tab", {name:/Blocked/}).getAttribute("aria-selected") === "true", "Blocked deep link selects the tab");
  const action = operation => page.evaluate(operation => api(operation, model.project.id), operation);
  const created = await action({action:"create",title:"Waiting for deployment access",body:"## Restore access\n\nReview the failed session and ask the owner for help.",labels:[]});
  const number = created.issue.number;
  await page.goto(base + `&issue=${number}`);
  await page.getByRole("button", {name:"Block issue",exact:true}).waitFor();
  await page.getByLabel("Your comment").fill("The owner needs to restore deployment access.");
  await page.getByRole("button", {name:"Block issue",exact:true}).click();
  await page.locator("#confirm-dialog[open]").waitFor();
  check((await page.locator("#confirm-dialog").innerText()).includes("Blocking should be rare"), "Warning asks for effort and help before blocking");
  for (const scheme of ["light", "dark"]) {
    await page.emulateMedia({colorScheme:scheme});
    for (const width of [1440,390,320]) {
      await page.setViewportSize({width,height:900});
      check(await page.locator("#confirm-dialog").evaluate(el => el.scrollWidth <= el.clientWidth), `Warning fits ${scheme}/${width}`);
      await page.screenshot({path:`output/playwright/issue65/${scheme}-${width}-warning.png`});
    }
  }
  await page.emulateMedia({colorScheme:"light"});
  await page.setViewportSize({width:1440,height:900});
  await page.keyboard.press("Escape");
  check(await page.getByLabel("Your comment").inputValue() === "The owner needs to restore deployment access.", "Cancelling preserves the explanation");
  const fail = route => route.request().postDataJSON()?.operation.action === "block"
    ? route.fulfill({status:500,contentType:"application/json",body:JSON.stringify({ok:false,error:{message:"Synthetic block failure"}})}) : route.continue();
  await page.route("**/api/action", fail);
  await page.getByRole("button", {name:"Block issue",exact:true}).click();
  await page.locator("#confirm-dialog").getByRole("button", {name:"Block issue",exact:true}).click();
  await page.getByText("Synthetic block failure", {exact:true}).waitFor();
  check(await page.getByRole("button", {name:"Block issue",exact:true}).isEnabled(), "Failed transition can be retried");
  check(await page.getByLabel("Your comment").inputValue() === "The owner needs to restore deployment access.", "Failed transition preserves explanation");
  await page.unroute("**/api/action", fail);
  await page.getByRole("button", {name:"Block issue",exact:true}).click();
  await page.locator("#confirm-dialog").getByRole("button", {name:"Block issue",exact:true}).click();
  await page.locator(".blocked-notice").waitFor();
  check(await page.locator(".state-pill.blocked").innerText() === "Blocked", "Detail identifies blocked status");
  await page.waitForFunction(() => document.querySelector("#comment-body")?.value === "");
  check(await page.getByLabel("Your comment").inputValue() === "", "Saved explanation is cleared from composer");
  check(await page.locator("#comments .comment-body").last().innerText() === "The owner needs to restore deployment access.", "Explanation saved with block");
  check((await page.locator(".readiness-status").innerText()).includes("pickup paused"), "Sidebar explains worker eligibility");
  check(await page.getByRole("button", {name:"Assign to Boss",exact:true}).count() === 0, "Blocked issue cannot be claimed");
  for (const scheme of ["light", "dark"]) {
    await page.emulateMedia({colorScheme:scheme,reducedMotion:"reduce"});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:900});
      await page.goto(base + `&state=blocked&issue=${number}`);
      await page.locator(".blocked-notice").waitFor();
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `Detail fits ${scheme}/${width}`);
      check(await page.locator(".blocked-notice").evaluate(el => el.scrollWidth <= el.clientWidth), `Blocked notice fits ${scheme}/${width}`);
      await page.screenshot({path:`output/playwright/issue65/${scheme}-${width}-detail.png`,fullPage:true});
      await page.getByRole("button", {name:"All issues",exact:true}).click();
      await page.locator(`.issue-row[data-issue-number="${number}"]`).waitFor();
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `List fits ${scheme}/${width}`);
      check(await page.locator('.state-tabs button[data-state="deleted"]').evaluate(el => el.getBoundingClientRect().right <= innerWidth), `All status tabs visible ${scheme}/${width}`);
      check(await page.locator('.state-tabs').evaluate(el => el.scrollWidth <= el.clientWidth), `Status tabs are not clipped ${scheme}/${width}`);
      check(await page.locator(`.issue-row[data-issue-number="${number}"] .issue-state.blocked`).isVisible(), `Blocked row has distinct icon ${scheme}/${width}`);
      await page.screenshot({path:`output/playwright/issue65/${scheme}-${width}-list.png`,fullPage:true});
    }
  }
  await page.getByRole("tab", {name:/Blocked/}).focus();
  await page.keyboard.press("ArrowRight");
  await page.waitForFunction(() => model.route.state === "closed");
  check(await page.getByRole("tab", {name:/Closed/}).getAttribute("aria-selected") === "true", "Keyboard cycles all four states");
  await page.goto(base + `&state=blocked&issue=${number}`);
  await page.locator(".blocked-notice").getByRole("button", {name:"Reopen issue",exact:true}).click();
  await page.locator(".state-pill.open").waitFor();
  check(await page.locator(".blocked-notice").count() === 0, "Reopening removes blocked notice");
  await page.getByRole("button", {name:"Close issue",exact:true}).click();
  await page.locator(".state-pill.closed").waitFor();
  check(await page.getByRole("button", {name:"Block issue",exact:true}).count() === 0, "Closed issue must be reopened before blocking");
  check(errors.length === 0, `No browser exceptions: ${errors}`);
  return {checks:checks.length,errors};
}
