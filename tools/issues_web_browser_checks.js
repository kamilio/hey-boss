// Run through playwright-cli run-code --filename tools/issues_web_browser_checks.js
// Requires the isolated fixtures from tools/seed_issues_web.py on port 4782.
async (page) => {
  await page.emulateMedia({ colorScheme: "light" });
  const base = "http://127.0.0.1:4782/";
  let bossName;
  const checks = [];
  const check = async (condition, name) => {
    if (!condition) throw new Error(name);
    checks.push(name);
    await page.evaluate(
      (checks) =>
        sessionStorage.setItem("issues-qa-progress", JSON.stringify(checks)),
      checks,
    );
  };
  const visible = (locator) => locator.waitFor({ state: "visible" });
  const wait = (predicate) => page.waitForFunction(predicate);
  page.on("dialog", (dialog) => dialog.accept().catch(() => {}));
  const browserErrors = [];
  page.on("pageerror", (error) => browserErrors.push(error.message));
  try {
    await page.goto(base);
    await page.waitForFunction(() => model.csrf && model.project && (model.signature || model.detail));
    bossName = await page.evaluate(() => model.boss.name);
    await page.evaluate(() => {
      for (const key of Object.keys(localStorage))
        if (key.startsWith("hey-boss-issues")) localStorage.removeItem(key);
    });
    await page.goto(base + "#project=github.com%2Fexample%2Fhey-boss");
    await page.setViewportSize({ width: 1440, height: 1000 });
    await visible(
      page.getByRole("link", {
        name: "Reconnect automatically after waking from sleep",
        exact: true,
      }),
    );
    await check(
      (await page.locator("#issue-list article").count()) === 11,
      "Initial project shows 11 open issues",
    );
    await page
      .getByRole("searchbox", { name: "Search issues", exact: true })
      .fill("reconnect");
    await wait(
      () => document.querySelectorAll("#issue-list article").length === 1,
    );
    await check(
      (await page.locator("#issue-list").textContent()).includes(
        "Reconnect automatically",
      ),
      "Search filters by title",
    );
    await page
      .getByRole("searchbox", { name: "Search issues", exact: true })
      .fill("");
    await wait(
      () => document.querySelectorAll("#issue-list article").length === 11,
    );
    await page
      .getByRole("combobox", { name: "Filter by label" })
      .selectOption("bug");
    await wait(
      () => document.querySelectorAll("#issue-list article").length === 4,
    );
    await check(
      (await page
        .locator("#issue-list .label")
        .filter({ hasText: /^bug$/ })
        .count()) === 4,
      "Label filter selects only matching issues",
    );
    await page
      .getByRole("combobox", { name: "Filter by label" })
      .selectOption("");
    await wait(
      () => document.querySelectorAll("#issue-list article").length === 11,
    );
    await page
      .getByRole("combobox", { name: "Filter by assignee" })
      .selectOption("mine");
    await wait(
      () => document.querySelectorAll("#issue-list article").length === 1,
    );
    await check(
      (await page.locator("#issue-list").textContent()).includes(
        "Reduce the time",
      ),
      "Assigned-to-me filter uses the browser identity",
    );
    await page
      .getByRole("combobox", { name: "Filter by assignee" })
      .selectOption("unassigned");
    await wait(
      () => document.querySelectorAll("#issue-list article").length === 6,
    );
    await check(
      (await page.locator("#issue-list .avatar").count()) === 0,
      "Unassigned filter excludes every claimed issue",
    );
    await page
      .getByRole("combobox", { name: "Filter by assignee" })
      .selectOption("all");
    await visible(page.getByRole("tab", { name: "Closed 2", exact: true }));
    await page.getByRole("tab", { name: "Closed 2", exact: true }).click();
    await wait(
      () => document.querySelectorAll("#issue-list article").length === 2,
    );
    await check(
      (await page.locator("#issue-list").textContent()).includes(
        "Make closed issues searchable",
      ),
      "Closed tab shows completed work",
    );
    const switchProject = async (name, id = null) => {
      await page.locator("#project-trigger").click();
      await page.getByRole("searchbox", { name: "Find a project" }).fill(name);
      const option = page
        .locator(".project-option")
        .filter({
          has: page.locator("strong", {
            hasText: new RegExp(
              "^" + name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + "$",
            ),
          }),
        });
      await (id ? page.locator('.project-option[data-project="'+id+'"]') : option).click();
      await wait(
        () => !document.querySelector("#project-menu").hidden === false,
      );
    };
    await switchProject("toolcraft");
    await visible(
      page.getByRole("link", {
        name: "Add a compact task inspector",
        exact: true,
      }),
    );
    await check(
      (await page.locator("#issue-list article").count()) === 3,
      "Project switch changes issue scope",
    );
    await switchProject("Empty project");
    await visible(
      page.getByRole("heading", {
        name: "A clear place to start",
        exact: true,
      }),
    );
    await check(true, "An empty project has a useful empty state");
    await page.locator("#project-trigger").click();
    await page
      .getByRole("button", { name: "New project", exact: true })
      .click();
    const projectName = "Browser QA " + Date.now();
    await page
      .getByRole("textbox", { name: "Project name", exact: true })
      .fill(projectName);
    await page.getByRole("button", { name: "Continue", exact: true }).click();
    await visible(page.getByRole("dialog", { name: "New issue", exact: true }));
    const title = "Verify the complete issue workflow";
    const body =
      "## Acceptance criteria\n\n- [ ] Keep drafts safe\n- [x] Render **Markdown**\n\n| Check | Result |\n|---|---|\n| Browser | Ready |\n\n<script>window.injected=true</script>\n\n[Unsafe](javascript:alert(1))";
    await page.getByRole("textbox", { name: "Title", exact: true }).fill(title);
    await page
      .getByRole("textbox", { name: "Issue description", exact: true })
      .fill(body);
    await page
      .getByRole("textbox", { name: /Labels/ })
      .fill("bug, needs-review");
    await page.locator("#editor-preview").click();
    await visible(page.locator("#editor-rendered table"));
    await check(
      (await page.locator("#editor-rendered strong").textContent()) ===
        "Markdown",
      "Markdown preview renders tables and inline formatting",
    );
    await check(
      await page.evaluate(
        () =>
          !window.injected &&
          !document.querySelector("#editor-rendered script") &&
          !document.querySelector('#editor-rendered a[href^="javascript:"]'),
      ),
      "Raw HTML and javascript links remain inert",
    );
    await page
      .getByRole("button", { name: "Create issue", exact: true })
      .click();
    await visible(page.getByRole("link", { name: title, exact: true }));
    await check(await page.locator("#list-view").isVisible(), "Creating an issue stays on the list");
    await page.getByRole("link", { name: title, exact: true }).click();
    await visible(
      page.getByRole("heading", { name: title + " #1", exact: true }),
    );
    await check(
      (await page.locator(".side-project").textContent()).includes(projectName),
      "New project and its first issue are saved together",
    );
    await page
      .getByRole("button", { name: "Assign to " + bossName, exact: true })
      .click();
    await visible(page.getByRole("button", { name: "Unassign", exact: true }));
    await page.getByRole("button", { name: "Unassign", exact: true }).click();
    await visible(
      page.getByRole("button", { name: "Assign to " + bossName, exact: true }),
    );
    await check(true, "Claim and unassign update ownership without closing");
    await page
      .getByRole("textbox", { name: "Your comment", exact: true })
      .fill("Draft survives navigation");
    await page.getByRole("button", { name: "All issues", exact: true }).click();
    await page.getByRole("link", { name: title, exact: true }).click();
    await visible(
      page.getByRole("textbox", { name: "Your comment", exact: true }),
    );
    await check(
      (await page
        .getByRole("textbox", { name: "Your comment", exact: true })
        .inputValue()) === "Draft survives navigation",
      "Comment draft survives leaving and reopening the issue",
    );
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await visible(
      page
        .locator("#comments .comment-card")
        .filter({ hasText: "Draft survives navigation" }),
    );
    await check(
      (await page
        .getByRole("textbox", { name: "Your comment", exact: true })
        .inputValue()) === "",
      "A saved comment clears its draft",
    );
    await page.getByRole("button", { name: "Edit", exact: true }).click();
    await page
      .getByRole("textbox", { name: "Title", exact: true })
      .fill(title + " — edited");
    const bootstrap = await (
      await page.request.get(base + "api/bootstrap")
    ).json();
    const project = await page.evaluate(() =>
      new URLSearchParams(location.hash.slice(1)).get("project"),
    );
    const external = await page.request.post(base + "api/action", {
      headers: { "X-Hey-Boss-CSRF": bootstrap.csrf },
      data: {
        project,
        operation: {
          action: "edit",
          number: 1,
          title: "Changed by a concurrent writer",
          body: null,
          add_labels: [],
          remove_labels: [],
          if_version: null,
        },
        request_id: null,
      },
    });
    await check(
      external.ok(),
      "Concurrent writer updates the underlying issue",
    );
    await page
      .getByRole("button", { name: "Save changes", exact: true })
      .click();
    await visible(page.locator("#editor-conflict"));
    await check(
      (await page.locator("#conflict-latest").textContent()).includes(
        "Changed by a concurrent writer",
      ),
      "Stale edits display the latest version without losing the draft",
    );
    await check(
      (await page
        .getByRole("textbox", { name: "Title", exact: true })
        .inputValue()) ===
        title + " — edited",
      "Conflicting draft stays intact",
    );
    await page
      .getByRole("button", {
        name: "Save my draft over this version",
        exact: true,
      })
      .click();
    await visible(
      page.getByRole("heading", { name: title + " — edited #1", exact: true }),
    );
    await check(
      true,
      "Conflict recovery requires an explicit replacement of the displayed version",
    );
    const lost = "A committed comment whose first response was lost";
    let dropped = false;
    await page.route("**/api/action", async (route) => {
      const data = route.request().postDataJSON();
      if (
        !dropped &&
        data.operation?.action === "comment" &&
        data.operation.body === lost
      ) {
        dropped = true;
        await route.fetch();
        await route.abort("failed");
      } else await route.continue();
    });
    await page
      .getByRole("textbox", { name: "Your comment", exact: true })
      .fill(lost);
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await visible(page.locator("#comment-error"));
    await check(
      dropped,
      "A lost response after commit is reported as an uncertain result",
    );
    await page.unroute("**/api/action");
    await page.locator("#comment-body").evaluate((el) => {
      el.value = "";
    }); // Preserve the stored draft while avoiding a browser-native unload prompt in the test runner.
    await page.reload();
    await visible(
      page.getByRole("textbox", { name: "Your comment", exact: true }),
    );
    await check(
      (await page
        .getByRole("textbox", { name: "Your comment", exact: true })
        .inputValue()) === lost,
      "Uncertain comment draft survives a full page reload",
    );
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await wait(() => document.querySelector("#comment-body")?.value === "");
    await check(
      (await page
        .locator("#comments .comment-card")
        .filter({ hasText: lost })
        .count()) === 1,
      "Persisted retry ID prevents duplicate comments after reload",
    );
    await page
      .getByRole("button", { name: "Close issue", exact: true })
      .click();
    await visible(
      page.getByRole("button", { name: "Reopen issue", exact: true }),
    );
    await check(
      (await page.locator(".detail-meta .state-pill").textContent()) ===
        "Closed",
      "Closing marks the issue complete",
    );
    await page
      .getByRole("button", { name: "Reopen issue", exact: true })
      .click();
    await visible(
      page.getByRole("button", { name: "Close issue", exact: true }),
    );
    await page
      .getByRole("button", { name: "Delete issue", exact: true })
      .click();
    await page
      .getByRole("dialog", { name: "Delete this issue?", exact: true })
      .getByRole("button", { name: "Delete issue", exact: true })
      .click();
    await visible(page.locator(".state-pill.deleted"));
    await page
      .getByRole("button", { name: "Restore issue", exact: true })
      .first()
      .click();
    await visible(
      page.getByRole("button", { name: "Close issue", exact: true }),
    );
    await check(
      (await page.locator("#comments .comment-card").count()) === 2,
      "Delete and restore preserve all comments",
    );
    await page
      .getByRole("button", { name: "View activity", exact: true })
      .click();
    await visible(page.locator(".timeline-item").first());
    await check(
      (await page.locator(".timeline").textContent()).includes(
        "restored this issue",
      ),
      "Activity records the entire lifecycle",
    );
    await page.getByRole("button", { name: "All issues", exact: true }).click();
    await page.keyboard.press("n");
    await visible(page.getByRole("dialog", { name: "New issue", exact: true }));
    await page
      .getByRole("textbox", { name: "Title", exact: true })
      .fill("Keyboard draft");
    await page.keyboard.press("Escape");
    await page.keyboard.press("n");
    await check(
      (await page
        .getByRole("textbox", { name: "Title", exact: true })
        .inputValue()) === "Keyboard draft",
      "Keyboard shortcut and Escape preserve an unsaved issue draft",
    );
    await page.keyboard.press("Escape");
    await page.keyboard.press("/");
    await check(
      await page
        .getByRole("searchbox", { name: "Search issues", exact: true })
        .evaluate((el) => el === document.activeElement),
      "Slash focuses issue search",
    );
    await page
      .getByRole("searchbox", { name: "Search issues", exact: true })
      .blur();
    await switchProject("Scale test");
    await wait(
      () => document.querySelectorAll("#issue-list article").length === 5000,
    );
    await check(
      (await page.locator("#issue-list article").count()) === 5000,
      "All 5,000 issues appear on one page",
    );
    await check(
      await page.getByRole("link", { name: "Performance fixture 5000", exact: true }).count() === 1 &&
        await page.getByRole("button", { name: "Next", exact: true }).count() === 0,
      "The last issue is present without pagination",
    );
    await switchProject("hey-boss", "github.com/example/hey-boss");
    await visible(
      page.getByRole("link", {
        name: "Reconnect automatically after waking from sleep",
        exact: true,
      }),
    );
    if (
      await page
        .getByRole("button", { name: "Dismiss notification", exact: true })
        .isVisible()
    )
      await page
        .getByRole("button", { name: "Dismiss notification", exact: true })
        .click();
    await page.screenshot({
      path: "output/playwright/issues-desktop-light.png",
      fullPage: true,
    });
    await page.emulateMedia({ colorScheme: "dark" });
    await page.screenshot({
      path: "output/playwright/issues-desktop-dark.png",
      fullPage: true,
    });
    await page
      .getByRole("link", {
        name: "Reconnect automatically after waking from sleep",
        exact: true,
      })
      .click();
    await visible(
      page.getByRole("heading", {
        name: "Reconnect automatically after waking from sleep #1",
        exact: true,
      }),
    );
    await page.screenshot({
      path: "output/playwright/issues-detail-dark.png",
      fullPage: true,
    });
    await page.emulateMedia({ colorScheme: "light" });
    await page.screenshot({
      path: "output/playwright/issues-detail-light.png",
      fullPage: true,
    });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.screenshot({
      path: "output/playwright/issues-mobile-detail.png",
      fullPage: true,
    });
    await page.getByRole("button", { name: "All issues", exact: true }).click();
    await visible(
      page.getByRole("link", {
        name: "Reconnect automatically after waking from sleep",
        exact: true,
      }),
    );
    await page.screenshot({
      path: "output/playwright/issues-mobile-light.png",
      fullPage: true,
    });
    await check(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
      "Mobile layout has no horizontal overflow",
    );
    await page.locator("#project-trigger").click();
    const menu = await page.locator("#project-menu").boundingBox();
    await check(
      menu.x >= 0 && menu.x + menu.width <= 390,
      "Mobile project switcher stays inside the viewport",
    );
    await page.screenshot({
      path: "output/playwright/issues-mobile-projects.png",
    });
    await page.keyboard.press("Escape");
    await page.emulateMedia({ colorScheme: "dark" });
    await page.screenshot({
      path: "output/playwright/issues-mobile-dark.png",
      fullPage: true,
    });
    await page.locator("#new-issue").click();
    await visible(page.getByRole("dialog", { name: "New issue", exact: true }));
    await page.screenshot({
      path: "output/playwright/issues-mobile-editor.png",
    });
    await check(
      await page.evaluate(
        () =>
          document.querySelector("#editor-dialog").getBoundingClientRect()
            .right <= innerWidth,
      ),
      "Mobile editor fits the viewport",
    );
    await page.keyboard.press("Escape");
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.emulateMedia({ colorScheme: "light" });
    await check(
      browserErrors.length === 0,
      "No uncaught browser JavaScript errors: " + browserErrors.join("; "),
    );
    await page.evaluate(
      (checks) =>
        sessionStorage.setItem(
          "issues-qa-result",
          JSON.stringify({ passed: checks.length, checks }),
        ),
      checks,
    );
    return { passed: checks.length, checks, browserErrors };
  } catch (error) {
    await page.evaluate(
      (message) =>
        sessionStorage.setItem(
          "issues-qa-result",
          JSON.stringify({ error: message }),
        ),
      error.message,
    );
    throw error;
  }
}
