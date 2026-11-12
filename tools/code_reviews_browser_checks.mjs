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
  await page.getByText("No reviews yet.", { exact: false }).waitFor();
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
  op({
    ...first,
    number: 72,
    title: "Keep existing reminder labels",
    comments: [],
    verdict: "APPROVE",
    body: "The change preserves existing labels.",
  });
  await page.locator("#reviews-refresh").click();
  await page.getByRole("button", { name: /#71/ }).click();
  await page
    .getByRole("button", { name: /#71/, expanded: true })
    .waitFor();
  assert.equal(await page.getByText(first.title, { exact: true }).count(), 1);
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
  const row = page.getByRole("button", { name: /#71/ });
  await row.click();
  assert.equal(await row.getAttribute("aria-expanded"), "false");
  assert.equal(await page.locator("#reviews-detail").count(), 0);
  await row.press("Enter");
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
      assert.ok(
        await page
          .locator("#reviews-list .review-row")
          .first()
          .evaluate(
            (el) =>
              el.getBoundingClientRect().right <=
              el.closest("#reviews-list").getBoundingClientRect().right + 1,
          ),
        "Queue rows must fit inside the list",
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
  assert.equal(op({ command: "view", id: saved.id }).review.stage, "submitted");
  await page.getByRole("button", { name: /#72/ }).click();
  await page.getByRole("button", { name: "Approve and publish" }).waitFor();
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
