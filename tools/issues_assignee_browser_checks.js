// Start on a synthetic project with issues 1: agent, 2: Boss, 3: backlog, 4: closed.
// Run using playwright-cli run-code --filename tools/issues_assignee_browser_checks.js.
async (page) => {
  const checks = [],
    errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const check = (value, name) => {
    if (!value) throw Error(name);
    checks.push(name);
  };
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.emulateMedia({ colorScheme: "light" });
  await page.reload();
  await page.waitForFunction(() => model.issues.length === 3);
  const project = await page.evaluate(() => model.project.id);
  const base = "http://127.0.0.1:4782/#project=" + encodeURIComponent(project);
  const rows = () => page.locator(".issue-row");
  const waitRows = async (numbers) => {
    await page.waitForFunction(
      (numbers) => model.issues.map((i) => i.number).join(",") === numbers,
      numbers.join(","),
    );
  };
  const hit = async (locator) =>
    locator.evaluate((el) => {
      const box = el.getBoundingClientRect(),
        target = document.elementFromPoint(
          box.x + box.width / 2,
          box.y + box.height / 2,
        );
      return target === el || el.contains(target);
    });
  check(
    await page.evaluate(() => model.actor.id === "human:boss"),
    "Web always acts as Boss",
  );
  check(
    (await page.locator("#self-avatar").getAttribute("title")) ===
      "Boss · human:boss",
    "Header identifies Boss",
  );
  const ready = page.locator(
    '[data-issue-number="1"] a[aria-label="Filter by label ready"]',
  );
  check(await hit(ready), "Label link receives pointer above row overlay");
  await ready.click();
  await waitRows([1, 2]);
  check(
    (await page.locator("#label-filter").inputValue()) === "ready",
    "Clicking label filters and updates dropdown",
  );
  check(
    (await page.url()).includes("label=ready"),
    "Label filter has a shareable URL",
  );
  const agent = page.locator('[data-issue-number="1"] .list-assignee-filter');
  check(await hit(agent), "Assignee receives pointer above row overlay");
  await agent.click();
  await waitRows([1]);
  check(
    (await page.locator("#owner-filter").inputValue()) === "codex:filter-qa",
    "Clicking assignee applies exact session filter",
  );
  check(
    (await page.locator("#label-filter").inputValue()) === "ready",
    "Assignee click retains label filter",
  );
  check(
    await page.evaluate(() => model.route.issue === null),
    "Filter click stays in list view",
  );
  await page.reload();
  await waitRows([1]);
  check(
    (await page.locator("#owner-filter").inputValue()) === "codex:filter-qa",
    "Assignee and label filters survive reload",
  );
  await page.locator("#owner-filter").selectOption("all");
  await waitRows([1, 2]);
  const boss = page.locator('[data-issue-number="2"] .list-assignee-filter');
  await boss.focus();
  await page.keyboard.press("Enter");
  await waitRows([2]);
  check(
    (await page.locator("#owner-filter").inputValue()) === "human:boss",
    "Keyboard activation filters by Boss",
  );
  await page.locator("#owner-filter").selectOption("mine");
  await waitRows([2]);
  check(
    (await rows().count()) === 1,
    "Assigned to me means Boss in the web app",
  );
  await page.locator("#owner-filter").selectOption("unassigned");
  await waitRows([]);
  await page.locator("#label-filter").selectOption("");
  await waitRows([3]);
  check(
    (await rows().count()) === 1,
    "Unassigned combines correctly with labels",
  );
  await page.locator("#owner-filter").selectOption("all");
  await waitRows([1, 2, 3]);
  await page.locator('[aria-label="Filter by label needs & review"]').click();
  await waitRows([1]);
  check(
    (await page.locator("#label-filter").inputValue()) === "needs & review",
    "Punctuation in labels round-trips through URL",
  );
  await page.locator("#label-filter").selectOption("");
  await waitRows([1, 2, 3]);
  await page.locator('[data-issue-number="1"] .issue-title').click();
  await page.locator('[data-action="unassign"]').waitFor();
  check(
    await page
      .getByRole("button", { name: "Assign to Boss", exact: true })
      .isVisible(),
    "Agent-owned issue offers Assign to Boss and Unassign",
  );
  await page.locator('[data-action="unassign"]').click();
  await page.locator("#confirm-cancel").click();
  check(
    (await page.evaluate(() => model.detail?.issue?.assignee)) ===
      "codex:filter-qa",
    "Cancelling unassign retains agent claim",
  );
  await page.locator('[data-action="unassign"]').click();
  await page.locator("#confirm-submit").click();
  await page.waitForFunction(() => model.detail?.issue?.assignee === null);
  const retained = await page.evaluate(() =>
    api({ action: "view", number: 1 }),
  );
  check(
    retained.issue.state === "open" &&
      retained.issue.body === "# Keep this content" &&
      retained.issue.labels.length === 2 &&
      retained.comments.length === 1 &&
      retained.issue.pull_requests.length === 1,
    "Unassign preserves state, Markdown, labels, comment, PR",
  );
  await page
    .getByRole("button", { name: "Assign to Boss", exact: true })
    .click();
  await page.waitForFunction(
    () => model.detail?.issue?.assignee === "human:boss",
  );
  check(
    !(await page.locator("#confirm-dialog").isVisible()),
    "Unassigned issue assigns directly to Boss",
  );
  await page.getByRole("button", { name: "Unassign", exact: true }).click();
  await page.waitForFunction(() => model.detail?.issue?.assignee === null);
  check(
    !(await page.locator("#confirm-dialog").isVisible()),
    "Boss unassigns own issue directly",
  );
  await page.locator("#self-avatar").click();
  await page.locator("#global-settings-trigger").click();
  await page.waitForFunction(() => !document.querySelector("#global-boss-name").disabled);
  await page.locator("#global-boss-name").fill("Alex <Boss>");
  await page.locator("#global-settings-submit").click();
  await page.locator("#global-settings-dialog").waitFor({ state: "hidden" });
  await page.getByRole("button", { name: "Assign to Alex <Boss>", exact: true }).waitFor();
  check(await page.locator("#project-boss-name").count() === 0, "Boss name belongs to global settings");
  await page.locator("#project-settings-trigger").click();
  await page.waitForFunction(() => !document.querySelector("#project-prompt").disabled);
  await page.locator("#project-prompt").fill("/goal");
  await page.waitForFunction(
    () =>
      !document.querySelector("#project-goal-indicator").hidden &&
      document
        .querySelector("#project-instructions-preview")
        .textContent.includes("Claim and implement"),
  );
  check(
    !(await page.locator("#project-settings-error").isVisible()),
    "Bare /goal preview still works with Boss settings",
  );
  await page.locator("#project-settings-form button[type=submit]").click();
  await page.locator("#project-settings-dialog").waitFor({ state: "hidden" });
  await page
    .getByRole("button", { name: "Assign to Alex <Boss>", exact: true })
    .waitFor();
  check(
    (await page.locator("#self-avatar").getAttribute("title")) ===
      "Alex <Boss> · human:boss",
    "Renamed identity updates immediately and escapes HTML",
  );
  await page.goto(base);
  await waitRows([1, 2, 3]);
  await page.waitForFunction(() => document.querySelector('[data-issue-number="2"] .list-assignee-filter > span:last-child')?.textContent === "Alex <Boss>");
  check(
    (await page
      .locator('[data-issue-number="2"] .list-assignee-filter > span:last-child')
      .innerText()) === "Alex <Boss>",
    "Existing Boss assignment displays renamed person",
  );
  await page.locator('[data-issue-number="2"] .list-assignee-filter').click();
  await waitRows([2]);
  check(
    (await page.locator("#owner-filter").inputValue()) === "human:boss",
    "Renaming preserves stable assignment filter",
  );
  await page.reload();
  await waitRows([2]);
  check(
    (await page.locator("#owner-filter").innerText()).includes("Alex <Boss>"),
    "Boss name persists after reload",
  );
  await page.goto(base + "&state=closed");
  await waitRows([4]);
  const closed = await page.evaluate(() => api({ action: "view", number: 4 }));
  const time = page.locator('[data-issue-number="4"] .issue-meta time');
  check(
    (await time.getAttribute("datetime")) ===
      new Date(closed.issue.closed_at).toISOString(),
    "Closed list uses closure time rather than creation time",
  );
  check(
    await hit(time),
    "Closure time is reachable for exact timestamp tooltip",
  );
  check(
    Boolean(await time.getAttribute("title")),
    "Closure tooltip includes exact date and time",
  );
  await page.setViewportSize({ width: 390, height: 844 });
  check(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    "Closed list fits mobile viewport",
  );
  await page.goto(base);
  await waitRows([1, 2, 3]);
  await page.locator('[data-issue-number="2"] .list-assignee-filter').click();
  await waitRows([2]);
  check((await rows().count()) === 1, "Assignee filter works on mobile");
  await page.goto(base + "&issue=2");
  await page.getByRole("button", { name: "Unassign", exact: true }).waitFor();
  check(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    "Mobile detail and assignment controls fit viewport",
  );
  await page.locator("#project-settings-trigger").click();
  await page.waitForFunction(
    () => !document.querySelector("#project-prompt").disabled,
  );
  check(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    "Project settings fit mobile viewport",
  );
  await page.locator("#project-settings-cancel").click();
  await page.emulateMedia({ colorScheme: "dark" });
  check(
    await page
      .getByRole("button", { name: "Unassign", exact: true })
      .isVisible(),
    "Assignment controls work in automatic dark appearance",
  );
  check(errors.length === 0, "No JavaScript runtime errors");
  await page.screenshot({
    path:
      "output/playwright/boss-" +
      page.context().browser().browserType().name() +
      ".png",
    fullPage: true,
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  return { passed: checks.length, checks };
}
