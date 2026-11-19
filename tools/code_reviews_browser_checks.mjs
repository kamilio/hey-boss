// Isolated real SQLite, web API and browser; never submits anything to GitHub.
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, mkdirSync } from "node:fs";
import { resolve, join } from "node:path";
import { spawn, spawnSync } from "node:child_process";
const { chromium } = await import(
  process.env.PLAYWRIGHT_MODULE || "playwright"
);
const bin = resolve(process.argv[2]),
  root = mkdtempSync(resolve(".review-browser-")),
  project = "github.com/example/time";
const env = {
  ...process.env,
  HEY_BOSS_AGENT_ID: "human:qa",
  HEY_BOSS_ISSUE_DB: join(root, "issues.db"),
  HEY_BOSS_FLEET_STATE: root,
  TMPDIR: root,
};
for (const k of [
  "HEY_BOSS_ISSUE_HOST",
  "HEY_BOSS_ISSUE_PROJECT",
  "HEY_BOSS_STATE_DIR",
])
  delete env[k];
const run = (args, input) => {
  const r = spawnSync(bin, args, {
    cwd: root,
    env,
    input: input && JSON.stringify(input),
    encoding: "utf8",
  });
  assert.equal(r.status, 0, r.stdout + r.stderr);
  return JSON.parse(r.stdout);
};
const issue = ["issue", "--project", project, "--json"],
  op = (o) => run([...issue, "code-review"], o);
let web, browser;
try {
  web = spawn(bin, [...issue, "web", "--port", "0", "--no-discovery"], {
    env,
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const info = await new Promise((yes, no) => {
    let output = "";
    const timer = setTimeout(() => no(Error("Web startup timed out")), 30000);
    web.stdout.on("data", (b) => {
      output += b;
      if (output.includes("\n")) {
        clearTimeout(timer);
        yes(JSON.parse(output.split("\n")[0]));
      }
    });
    web.on("error", no);
  });
  run([
    "project",
    "init",
    "--project",
    project,
    "--prs",
    "true",
    "--worktree",
    "false",
    "--yes",
    "--json",
  ]);
  browser = await chromium.launch({ channel: "chrome", headless: true });
  const page = await browser.newPage(),
    errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  const url = info.url + "code-reviews#" + new URLSearchParams({ project });
  await page.goto(url);
  await page.getByText("Code Reviews is disabled for this project.").waitFor();
  assert.equal(await page.locator("#nav-code-reviews").isVisible(), false);
  run([...issue, "settings", "set", "--code-reviews-enabled"]);
  await page.reload();
  await page.getByText("No pending reviews.", { exact: true }).waitFor();
  const first = {
    command: "record",
    number: 71,
    head: "a".repeat(40),
    title:
      "Preserve the selected time zone when a scheduled reminder crosses midnight",
    comments: [
      {
        path: "scheduler/reminders.rs",
        line: 42,
        side: "RIGHT",
        body: "Cover the boundary in a test.",
        snippet:
          "@@ -40,3 +40,4 @@\n let zone = job.time_zone;\n let next = reminder.next();\n+next.in_zone(zone)\n return next;",
      },
    ],
    verdict: "REQUEST_CHANGES",
    body: "A reminder scheduled just before midnight can appear on the wrong day.",
  };
  const patch = "diff --git a/scheduler/reminders.rs b/scheduler/reminders.rs\n--- a/scheduler/reminders.rs\n+++ b/scheduler/reminders.rs\n" + first.comments[0].snippet + "\ndiff --git a/scheduler/labels.rs b/scheduler/labels.rs\n--- a/scheduler/labels.rs\n+++ b/scheduler/labels.rs\n@@ -1 +1 @@\n-clear(labels);\n+preserve(labels);\n";
  await page.context().route("**/api/code-reviews/diff", route => route.fulfill({json:{ok:true,patch,truncated:false,author:"reminder-author"}}));
  const saved = op(first).review;
  const approval = op({
    ...first,
    number: 72,
    title: "Keep existing reminder labels",
    comments: [],
    verdict: "APPROVE",
    body: "The change preserves existing labels.",
  }).review;
  op({ ...first, number: 73, title: "Draft with inline feedback", verdict: null, body: "" });
  await page.locator("#reviews-refresh").click();
  const indexRow = page.getByRole("link", {name: new RegExp(first.title)});
  await indexRow.waitFor();
  assert.equal(await page.locator("#reviews-detail").count(), 0);
  assert.ok((await indexRow.locator("..").textContent()).includes(first.body));
  assert.ok((await indexRow.locator("..").textContent()).includes(first.comments[0].body));
  assert.ok((await indexRow.locator("..").textContent()).includes("1 inline comment"));
  const item = page.locator(".review-index-row").filter({ has: indexRow });
  assert.equal(await indexRow.getAttribute("href"), "/code-review#" + new URLSearchParams({project, review:saved.id}));
  assert.deepEqual(await item.locator("button").allTextContents(), ["Publish requested changes"]);
  const draft = page.locator(".review-index-row").filter({hasText:"#73"});
  assert.ok((await draft.textContent()).includes(first.comments[0].body));
  assert.equal(await draft.locator(".button.primary").count(), 0);
  await page.setViewportSize({width:320,height:844});
  assert.ok(await item.evaluate(el => el.getBoundingClientRect().right <= innerWidth));
  await page.setViewportSize({width:1280,height:900});
  await indexRow.click();
  const heading = page.getByRole("heading", {name:first.title, exact:true});
  await heading.waitFor();
  assert.equal(new URL(page.url()).pathname, "/code-review");
  assert.equal(await page.locator(".review-index-row").count(), 0);
  assert.equal(await page.getByRole("link", {name:"Open PR #71 ↗"}).getAttribute("href"), "https://github.com/example/time/pull/71");
  await page.getByText("Author: reminder-author", {exact:true}).waitFor();
  assert.equal(await page.locator(".review-file[open]").count(), 2, "Every changed file starts expanded");
  assert.ok(await page.locator(".review-actions").evaluate(el => el.getBoundingClientRect().bottom <= document.querySelector(".review-editor").getBoundingClientRect().top), "Review actions precede the review body");
  await page.reload();
  await heading.waitFor();
  await page
    .getByRole("button", {
      name: "Edit comment on scheduler/reminders.rs line 42",
    })
    .click();
  await page
    .getByRole("textbox", { name: /comment on/ })
    .fill("Please test 23:59 in the selected time zone.");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await page
    .getByText("Please test 23:59 in the selected time zone.", { exact: true })
    .waitFor();
  await page.getByRole("button", { name: "Edit review comment" }).click();
  await page
    .getByRole("textbox", { name: "review comment" })
    .fill("Fix the midnight boundary before merging.");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await page
    .getByText("Fix the midnight boundary before merging.", { exact: true })
    .waitFor();
  const edited = op({ command: "view", id: saved.id }).review;
  assert.equal(
    edited.comments[0].body,
    "Please test 23:59 in the selected time zone.",
  );
  assert.equal(edited.body, "Fix the midnight boundary before merging.");
  assert.equal(
    await page.locator("pre .marked").textContent(),
    "42+next.in_zone(zone)",
  );
  assert.ok(
    (
      await page.locator(".review-comment header a").getAttribute("href")
    ).endsWith("R42"),
  );
  await page.goBack();
  await indexRow.waitFor();
  assert.equal(page.url(), url);
  await indexRow.press("Enter");
  await heading.waitFor();
  await page.getByText(edited.body, { exact: true }).waitFor();
  assert.ok(await page.getByRole("button", { name: "Publish requested changes" })
    .evaluate((el) => el.matches(".button.primary")));
  const snap = process.env.CODE_REVIEW_SCREENSHOTS;
  if (snap) mkdirSync(snap, { recursive: true });
  for (const width of [1440, 768, 390, 320])
    for (const colorScheme of ["light", "dark"]) {
      await page.setViewportSize({ width, height: width < 600 ? 844 : 960 });
      await page.emulateMedia({ colorScheme });
      assert.ok(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= innerWidth,
        ),
      );
      if (snap)
        await page.screenshot({
          path: join(snap, width + "-" + colorScheme + ".png"),
          fullPage: true,
        });
    }
  let requested = null;
  await page.route("**/api/code-reviews/publish", (r) => {
    requested = r.request().postDataJSON();
    op({command:"mark_outdated", id:saved.id, version:edited.version});
    return r.fulfill({
      status: 409,
      json: {
        ok: false,
        error: { message: "PR head changed; review the latest changes." },
      },
    });
  });
  await page.getByRole("button", { name: "Publish requested changes" }).click();
  await page
    .getByRole("alert")
    .getByText(/PR head changed/)
    .waitFor();
  assert.deepEqual(requested, {
    project,
    id: saved.id,
    version: edited.version,
    digest: edited.digest,
  });
  assert.equal(op({ command: "view", id: saved.id }).review.stage, "outdated");
  assert.equal(await page.getByRole("button",{name:"Publish requested changes"}).count(), 0);
  const archive = page.getByRole("button", {name:"Archive", exact:true});
  const pending = page.getByRole("button", {name:"Pending", exact:true});
  assert.equal(await archive.isVisible(), false);
  page.once("dialog", dialog => dialog.accept());
  await page.getByRole("button",{name:"Discard review"}).click();
  await page.getByText("Review discarded.", {exact:true}).waitFor();
  await page.getByText("#71 · Discarded").waitFor();
  await page.getByRole("link", {name:"Back to reviews"}).click();
  await pending.waitFor();
  assert.equal(await pending.getAttribute("aria-pressed"), "true");
  await page.getByText("#73 · Draft", {exact:false}).waitFor();
  assert.equal(await indexRow.count(), 0);
  const approve = page.getByRole("button", { name: "Approve and publish" });
  const latest = op({ command:"edit", id:approval.id, version:approval.version,
    comment:null, body:"Updated approval text." }).review;
  requested = null;
  await approve.click();
  await page.getByRole("alert").getByText(/review changed/i).waitFor();
  assert.equal(requested, null, "Never publish unseen edits");
  await page.locator("#reviews-refresh").click();
  await page.getByText(latest.body, {exact:true}).waitFor();
  await page.unroute("**/api/code-reviews/publish");
  await page.route("**/api/code-reviews/publish", (route) => {
    requested = route.request().postDataJSON();
    const claim = op({ command:"claim_publication", id:latest.id,
      version:latest.version, digest:latest.digest }).review;
    op({ command:"finish_publication", id:latest.id, version:claim.version,
      receipt:"https://github.com/example/time/pull/72#review-1", author:"qa" });
    return route.fulfill({ json:{ok:true} });
  });
  await approve.click();
  await page.getByText("Published to GitHub.", {exact:true}).waitFor();
  assert.deepEqual(requested, {project, id:latest.id, version:latest.version, digest:latest.digest});
  assert.equal(await page.locator("#reviews-detail").count(), 0);
  assert.equal(page.url(), url);
  assert.equal(await pending.getAttribute("aria-pressed"), "true");
  assert.equal(await approve.count(), 0);
  assert.equal(await page.getByText(latest.title,{exact:true}).count(), 0);
  await archive.click();
  await page.getByText(latest.title,{exact:true}).waitFor();
  assert.equal(await draft.count(), 0);
  await page.getByRole("link", {name:new RegExp(latest.title)}).click();
  await page.getByRole("link", {name:"View published review ↗"}).waitFor();
  await page.getByRole("link", {name:"Back to reviews"}).click();
  await page.getByText(latest.title, {exact:true}).waitFor();
  assert.equal(await archive.getAttribute("aria-pressed"), "true");
  const issueNumber = run([...issue, "create", "--title", "Check reminder labels"]).issue.number;
  run([...issue, "comment", String(issueNumber), "--body", "Discuss **reminder context** here."]);
  const issueUrl = info.url + "#" + new URLSearchParams({project, issue:issueNumber});
  for (const pr of ["example/time/pull/71","example/time/pull/72","example/time/pull/73","other/repo/pull/73"]) {
    run([...issue, "pr", "add", String(issueNumber), "https://github.com/" + pr]);
  }
  for (const address of [info.url + "#" + new URLSearchParams({project,state:"all"}), issueUrl]) {
    await page.goto(address);
    const links = page.getByRole("link", {name:/^Open pending review for PR/});
    await links.first().waitFor();
    assert.equal(await links.count(), 1, "Only the matching, unpublished PR has a review link");
    assert.equal(await links.innerText(), "Pending review");
    const row = address === issueUrl ? page.locator(".pr-link").filter({has:links}) : page.locator(".issue-row").filter({has:links});
    assert.equal(await row.locator('a[href="https://github.com/example/time/pull/73"]').count(), 1);
    await links.click();
    const preview = page.getByRole("dialog", {name:"Code review"}).frameLocator("iframe");
    await preview.getByRole("heading", {name:"Draft with inline feedback"}).waitFor();
    assert.equal(new URL(page.url()).pathname, "/");
    await preview.locator(".review-file pre + .review-comment").getByText(first.comments[0].body).waitFor();
    assert.match(await preview.locator(".rv-code-line.marked").innerText(), /next.in_zone/);
    assert.equal(await preview.locator(".review-discussion").count(), 0);
    await preview.getByRole("button",{name:"Edit review comment",exact:true}).focus();
    await page.keyboard.press("Escape");
    const [solo] = await Promise.all([page.context().waitForEvent("page"), links.click({modifiers:["Shift"]})]);
    await solo.waitForURL("**/code-review#**");
    await solo.locator(".review-discussion .markdown strong").waitFor();
    await solo.close();
  }
  await page.goto(issueUrl);
  const linked = page.locator("#issue-reviews");
  await linked.getByRole("button", {name:"Attach review"}).click();
  await linked.getByLabel("Review to attach").selectOption(String(approval.id));
  await linked.getByRole("button", {name:"Attach",exact:true}).click();
  const receipt = linked.getByRole("link",{name:"Published on GitHub"});
  await receipt.waitFor();
  assert.equal(await receipt.getAttribute("href"), "https://github.com/example/time/pull/72#review-1");
  const savedLink = linked.getByRole("link",{name:/#72/});
  assert.ok((await savedLink.getAttribute("href")).includes(String(approval.id)));
  if (snap) {
    await page.setViewportSize({width:1280,height:900});
    await page.emulateMedia({colorScheme:"light"});
    await linked.screenshot({path:join(snap,"issue-reviews.png")});
  }
  await page.goto(await savedLink.evaluate(a => a.href));
  await page.getByRole("link",{name:"View published review ↗"}).waitFor();
  assert.equal(new URL(page.url()).pathname, "/code-review");
  assert.equal(await page.locator(".review-index-row").count(), 0);
  await page.goto(issueUrl);
  await linked.getByRole("button",{name:"Unlink review for PR #72"}).click();
  await receipt.waitFor({state:"detached"});
  assert.equal(op({command:"view",id:approval.id}).review.stage, "published");
  run([...issue,"pr","add",String(issueNumber),"https://github.com/example/time/pull/74"]);
  const quick = op({...first,number:74,title:"Fix the reminder boundary",verdict:"APPROVE"}).review;
  await page.goto(info.url + "#" + new URLSearchParams({project,state:"all"}));
  await page.getByRole("link",{name:"Open pending review for PR #74"}).click();
  const modal = page.locator("#review-preview");
  const full = modal.frameLocator("iframe");
  await full.getByRole("heading",{name:quick.title}).waitFor();
  await page.unroute("**/api/code-reviews/publish");
  await page.route("**/api/code-reviews/publish", route => {
    const {id,version,digest} = route.request().postDataJSON();
    assert.deepEqual({id,version,digest},{id:quick.id,version:quick.version,digest:quick.digest});
    const claim = op({command:"claim_publication",id,version,digest}).review;
    op({command:"finish_publication",id,version:claim.version,receipt:"https://github.com/example/time/pull/74#review-3",author:"qa"});
    return route.fulfill({json:{ok:true}});
  });
  await page.route("**/api/code-reviews/publish", route => route.fulfill({status:409,json:{ok:false,error:{message:"Could not publish approval"}}}), {times:1});
  await full.getByRole("button",{name:"Approve and publish"}).click();
  await full.getByText("Could not publish approval", {exact:true}).waitFor();
  assert.ok(await modal.isVisible());
  assert.equal(op({command:"view",id:quick.id}).review.comments[0].snippet, first.comments[0].snippet);
  for (const [width,scheme] of [[320,"dark"],[1280,"light"]]) {
    await page.setViewportSize({width,height:768});
    await page.emulateMedia({colorScheme:scheme});
    const action = full.getByRole("button",{name:"Approve and publish"});
    await action.scrollIntoViewIfNeeded();
    assert.ok(await modal.evaluate(el => el.clientWidth >= innerWidth * .94));
    assert.ok(await action.evaluate(el => el.getBoundingClientRect().right <= innerWidth));
    if (snap) await modal.screenshot({path:join(snap,"preview-"+scheme+".png")});
  }
  await full.getByRole("button",{name:"Approve and publish"}).click();
  await modal.waitFor({state:"hidden"});
  await page.getByRole("link",{name:"Open pending review for PR #74"}).waitFor({state:"detached"});
  const published = op({command:"view",id:quick.id}).review;
  assert.equal(published.stage,"published");
  assert.deepEqual(published.comments, quick.comments.map(c => ({...c,snippet:""})));
  const sha = "c".repeat(40), message = "Keep reminder labels when the timezone changes";
  await page.evaluate(async ({project,issueNumber,sha,message}) => {
    await mutate({action:"add_commit",number:issueNumber,commit:sha,review:{
      repository:project,parent:"a".repeat(40),message:message+"\n\n**Summary:** Keep labels.\n\n<details>\n<summary>Details</summary>\n\n**Why:** Labels should persist.\n\n</details>",author:"QA",email:"qa@example.invalid",
      authored_at:"2026-11-16",patch:"diff --git a/labels.rs b/labels.rs\n--- a/labels.rs\n+++ b/labels.rs\n@@ -1 +1 @@\n-clear(labels);\n+preserve(labels);\n"
    }},project);
  },{project,issueNumber,sha,message});
  await page.reload();
  const chip = page.getByRole("link",{name:/Open commit ccccccc/});
  await chip.click();
  await full.getByRole("heading",{name:message}).waitFor();
  const controls = modal.locator(".dialog-heading .task-strip");
  await controls.waitFor();
  assert.equal(await full.locator(".task-strip").count(), 0, "Task controls leave the scrolling review");
  for (const [width,colorScheme] of [[320,"dark"],[1280,"light"]]) {
    await page.setViewportSize({width,height:768});
    await page.emulateMedia({colorScheme});
    assert.ok(await controls.evaluate(el => el.getBoundingClientRect().left >= 0 && el.getBoundingClientRect().right <= innerWidth), "Task controls fit the preview header");
    if (width === 1280) assert.ok(await controls.evaluate(el => Math.abs(el.getBoundingClientRect().top + el.clientHeight / 2 - (document.querySelector("#preview-title").getBoundingClientRect().top + document.querySelector("#preview-title").clientHeight / 2)) < 2), "Task controls sit beside Commit review");
    if (snap) await modal.screenshot({path:join(snap,"preview-header-"+colorScheme+".png")});
  }
  await full.locator("#message-body details summary").click();
  await full.locator("#message-body details strong").filter({hasText:"Why:"}).waitFor();
  await full.locator('.diff-row.addition code').filter({hasText:"+preserve(labels);"}).first().waitFor();
  assert.equal(await full.getByRole("button",{name:"Approve commit"}).count(),0);
  await controls.locator("#assign-me").click();
  await controls.locator("#unassign").click();
  await controls.locator("#task-owner").filter({hasText:"Unassigned"}).waitFor();
  await controls.locator("#make-ready").click();
  await controls.locator("#task-state").filter({hasText:"Ready"}).waitFor();
  if (snap) await modal.screenshot({path:join(snap,"preview-commit.png")});
  const commentCount = () => run([...issue, "view", String(issueNumber)]).comments.length;
  const beforeReopen = commentCount(), reopenOps = [];
  page.on("request", req => {
    if (new URL(req.url()).pathname === "/api/action") {
      const op = req.postDataJSON()?.operation;
      if (op?.number === issueNumber && ["comment", "rework"].includes(op.action)) reopenOps.push(op.action);
    }
  });
  await controls.locator("#rework").click();
  await full.getByLabel("Comment optional").fill("Keep the existing reminder labels.");
  await page.keyboard.press("Escape");
  assert.ok(await modal.isVisible(), "Escape closes only the comment dialog");
  assert.equal(await full.locator("#reopen-dialog").isVisible(), false);
  assert.equal(await controls.locator("#task-state").innerText(), "Ready");
  await controls.locator("#rework").click();
  for (const [width,colorScheme] of [[320,"dark"],[1280,"light"]]) {
    await page.setViewportSize({width,height:768});
    await page.emulateMedia({colorScheme});
    assert.ok(await full.locator("#reopen-dialog").evaluate(el => el.getBoundingClientRect().left >= 0 && el.getBoundingClientRect().right <= innerWidth));
    if (snap) await modal.screenshot({path:join(snap,"preview-reopen-"+colorScheme+".png")});
  }
  await page.route("**/api/action", route => {
    const op = route.request().postDataJSON()?.operation;
    if (op?.action === "comment") return route.fulfill({status:409,json:{ok:false,error:{message:"Comment could not be saved"}}});
    return route.fallback();
  });
  await full.getByLabel("Comment optional").press("Meta+Enter");
  await full.getByText("Comment could not be saved", {exact:true}).waitFor();
  assert.deepEqual(reopenOps, ["comment"], "Failed comments never reopen the task");
  assert.equal(commentCount(), beforeReopen);
  await page.unroute("**/api/action");
  await page.route("**/api/action", route => {
    if (route.request().postDataJSON()?.operation?.action === "rework") return route.fulfill({status:409,json:{ok:false,error:{message:"Reopen could not be saved"}}});
    return route.fallback();
  });
  await full.getByLabel("Comment optional").press("Meta+Enter");
  await full.getByText("Reopen could not be saved", {exact:true}).waitFor();
  assert.equal(commentCount(), beforeReopen + 1);
  assert.equal(await full.getByLabel("Comment optional").inputValue(), "", "Saved comments are not posted again on retry");
  assert.equal(await controls.locator("#task-state").innerText(), "Ready");
  await page.unroute("**/api/action");
  await full.getByLabel("Comment optional").press("Meta+Enter");
  await controls.locator("#task-state").filter({hasText:"Open"}).waitFor();
  assert.deepEqual(reopenOps, ["comment", "comment", "rework", "rework"]);
  assert.equal(commentCount(), beforeReopen + 1);
  assert.equal(run([...issue, "view", String(issueNumber)]).comments.at(-1).body, "Keep the existing reminder labels.");
  await controls.locator("#make-ready").click();
  await controls.locator("#task-state").filter({hasText:"Ready"}).waitFor();
  await controls.locator("#rework").click();
  await full.getByLabel("Comment optional").fill("   ");
  await full.locator('#reopen-form button[type="submit"]').click();
  await controls.locator("#task-state").filter({hasText:"Open"}).waitFor();
  assert.equal(commentCount(), beforeReopen + 1, "An empty comment reopens without posting");
  await page.keyboard.press("Escape");
  await modal.waitFor({state:"hidden"});
  const [commitPage] = await Promise.all([page.context().waitForEvent("page"),chip.click({modifiers:["Shift"]})]);
  await commitPage.getByRole("heading",{name:message}).waitFor();
  assert.ok(await commitPage.locator(".app-header .task-strip").isVisible(), "Standalone task controls appear in the header");
  await commitPage.close();
  // A long archive must not displace pending work or invalidate an old issue link.
  const seed = spawnSync("sqlite3", [env.HEY_BOSS_ISSUE_DB], {encoding:"utf8", input:
    `WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<205)
     INSERT INTO pr_code_reviews(id,project_id,number,head,title,stage,verdict,body,version,created_at,updated_at)
     SELECT -i,project_id,1000+i,head,'Archived review '||i,'discarded',verdict,'',1,created_at,updated_at+i FROM n,pr_code_reviews WHERE id=${approval.id};`});
  assert.equal(seed.status, 0, seed.stderr);
  await page.goto(url);
  await draft.waitFor();
  assert.equal(await page.locator(".review-index-row").count(), 1);
  await archive.click();
  await page.getByText("Archived review 205", {exact:true}).waitFor();
  assert.equal(await page.locator(".review-index-row").count(), 50);
  await page.getByRole("button", {name:"Show more reviews"}).click();
  await page.getByText("Archived review 155", {exact:true}).waitFor();
  assert.equal(await page.locator(".review-index-row").count(), 100);
  await page.goto(url + "&review=" + approval.id);
  await page.getByRole("link",{name:"View published review ↗"}).waitFor();
  assert.equal(new URL(page.url()).pathname, "/code-review");
  assert.equal(await page.locator(".review-index-row").count(), 0);
  // A link from another screen opens the detail without loading the queue.
  let listRequests = 0;
  page.on("request", req => {
    if (!req.url().endsWith("/api/action") || new URL(req.frame().url()).pathname !== "/code-review") return;
    const op = req.postDataJSON()?.operation;
    if (op?.action === "code_review" && op.operation.command === "list") listRequests++;
  });
  for (let number = 80; number < 95; number++)
    op({...first, number});
  await page.goto(info.url + "#" + new URLSearchParams({project,state:"all"}));
  await page.getByRole("link", {name:"Open pending review for PR #73"}).click();
  await full.getByRole("heading", {name:"Draft with inline feedback"}).waitFor();
  await page.getByRole("link",{name:"Full review"}).click();
  await page.locator("#reviews-detail").waitFor();
  assert.ok(await page.locator("#reviews-detail").evaluate(el => el.getBoundingClientRect().top < innerHeight));
  assert.equal(listRequests, 0);
  // Publishing from a standalone review leaves the receipt at the same address.
  await page.goto(url);
  await page.getByRole("link", {name:/#94 ·/}).click();
  const detailUrl = page.url();
  await page.unroute("**/api/code-reviews/publish");
  await page.route("**/api/code-reviews/publish", route => {
    const {id, version, digest} = route.request().postDataJSON();
    const claim = op({command:"claim_publication", id, version, digest}).review;
    op({command:"finish_publication", id, version:claim.version,
      receipt:"https://github.com/example/time/pull/94#review-2", author:"qa"});
    return route.fulfill({json:{ok:true}});
  });
  await page.getByRole("button", {name:"Publish requested changes"}).click();
  await page.getByRole("link", {name:"View published review ↗"}).waitFor();
  assert.equal(page.url(), detailUrl);
  assert.equal(await page.locator(".review-index-row").count(), 0);
  await page.getByRole("link", {name:"Back to reviews"}).click();
  await pending.waitFor();
  assert.equal(await pending.getAttribute("aria-pressed"), "true");
  assert.deepEqual(errors, []);
  console.log(
    "Code Reviews: disabled, empty, approval, changes requested, both edits, diff anchors, stale publication and 8 visual variants passed.",
  );
} finally {
  await browser?.close();
  if (web && web.exitCode === null) {
    const stopped = new Promise((r) => web.once("exit", r));
    web.kill("SIGINT");
    await stopped;
  }
  rmSync(root, { recursive: true, force: true });
}
