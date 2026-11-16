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
    await page.getByRole("heading", {name:"Draft with inline feedback"}).waitFor();
    assert.equal(new URL(page.url()).pathname, "/code-review");
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
    if (req.url().endsWith("/api/action") && req.postDataJSON()?.operation?.operation?.command === "list") listRequests++;
  });
  for (let number = 80; number < 95; number++)
    op({...first, number});
  await page.goto(info.url + "#" + new URLSearchParams({project,state:"all"}));
  await page.getByRole("link", {name:"Open pending review for PR #73"}).click();
  await page.getByRole("heading", {name:"Draft with inline feedback"}).waitFor();
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
