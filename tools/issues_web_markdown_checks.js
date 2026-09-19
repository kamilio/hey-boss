// Standalone playwright-cli run-code function. Requires the isolated server on
// 4782 with tests/fixtures/issues-markdown.md saved as named:Markdown QA issue 1.
async (page) => {
  const base = "http://127.0.0.1:4782/";
  const checks = [];
  const errors = [];
  const browserName = page.context().browser().browserType().name();
  page.on("dialog", (dialog) => dialog.accept().catch(() => {}));
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  const check = (condition, name) => {
    if (!condition) throw new Error(name);
    checks.push(name);
  };
  await page.goto(base + "#project=named%3AMarkdown%20QA&issue=1");
  await page.reload();
  await page.setViewportSize({ width: 1440, height: 1000 });
  const body = page.locator(".comment-body").first();
  await body.locator("h2").waitFor();
  const boot = await (await page.request.get(base + "api/bootstrap")).json();
  const fixture = await (
    await page.request.post(base + "api/action", {
      headers: { "X-Hey-Boss-CSRF": boot.csrf },
      data: {
        project: "named:Markdown QA",
        operation: { action: "view", number: 1 },
      },
    })
  ).json();
  const source = fixture.issue.body;
  check(
    (await body
      .locator('a[href^="https://github.com/poe-internal/poe2/pull/"]')
      .count()) === 2,
    "Both PR URLs from the reported issue become clickable links",
  );
  check(
    (await body.locator('a[href="https://example.com/a_(b)"]').count()) === 1,
    "Balanced parentheses remain in URLs and sentence punctuation stays outside",
  );
  check(
    (await body
      .locator('a[href="https://example.com/?first=1&second=2"]')
      .count()) === 1,
    "Query parameters survive Markdown parsing and HTML escaping",
  );
  check(
    (await body.locator('a[href="mailto:agent@example.com"]').count()) === 1 &&
      (await body.locator('a[href="https://www.example.com"]').count()) === 1,
    "Email and www addresses are linked",
  );
  check(
    await body
      .locator('a:not([href^="#"])')
      .evaluateAll((links) =>
        links.every(
          (a) =>
            a.target === "_blank" &&
            a.rel.includes("noopener") &&
            a.rel.includes("noreferrer"),
        ),
      ),
    "External links preserve the issue page and prevent opener access",
  );
  check(
    (await body.locator("code a").count()) === 0 &&
      (await body.locator("pre").count()) === 3,
    "Inline, fenced and indented code never receive automatic links",
  );
  check(
    await body
      .locator('input[type="checkbox"]')
      .evaluateAll(
        (inputs) =>
          inputs.length === 2 &&
          inputs.every(
            (input) => input.disabled && input.getAttribute("aria-label"),
          ) &&
          inputs[0].checked &&
          !inputs[1].checked,
      ),
    "Task lists retain checked state and accessible labels",
  );
  check(
    (await body.locator("ul ul li").count()) === 2 &&
      (await body.locator('ol[start="3"] ol').count()) === 1,
    "Nested lists and ordered-list start numbers are preserved",
  );
  check(
    await body
      .locator("th")
      .evaluateAll(
        (cells) =>
          cells.map((cell) => getComputedStyle(cell).textAlign).join() ===
          "left,center,right",
      ),
    "Table column alignment works under the real content security policy",
  );
  check(
    await body
      .locator('blockquote[class^="markdown-alert"]')
      .evaluateAll(
        (alerts) =>
          alerts
            .map((el) => getComputedStyle(el, "::before").content)
            .join() === '"Note","Tip","Important","Warning","Caution"' &&
          new Set(alerts.map((el) => getComputedStyle(el).borderLeftColor))
            .size === 5,
      ),
    "All five GitHub callouts have visible labels and semantic colors",
  );
  check(
    (await body.locator(".token-keyword").count()) > 0 &&
      (await body.locator(".token-insert").count()) === 1,
    "Fenced source code and diffs receive syntax colors",
  );
  check(
    (await body.locator("h4,h5,h6").count()) === 3 &&
      (await body.locator("br").count()) === 1,
    "Lower-level headings and hard line breaks render correctly",
  );

  await page.getByRole("button", { name: "Edit", exact: true }).click();
  await page.locator("#editor-preview").click();
  await page.locator("#editor-rendered h2").waitFor();
  check(
    (await page.locator("#editor-rendered").textContent()) ===
      (await body.textContent()),
    "Issue editor preview matches the saved Markdown content",
  );
  await page.keyboard.press("Escape");
  await page.locator("#comment-body").fill(source);
  await page.locator("#comment-preview").click();
  await page.locator("#comment-rendered h2").waitFor();
  check(
    (await page.locator("#comment-rendered").textContent()) ===
      (await body.textContent()),
    "Comment preview uses the same renderer as issue descriptions",
  );
  const previousComment = await page.evaluate(() => model.detail.comments.at(-1)?.id ?? null);
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await page.waitForFunction(
    (previous) =>
      model.detail?.comments.at(-1)?.id !== previous &&
      model.detail?.comments.at(-1)?.id != null &&
      document.querySelector("#comment-body")?.value === "",
    previousComment,
  );
  const comment = page.locator("#comments .comment-body").last();
  await comment.locator("h2").waitFor();
  check(
    (await comment.textContent()) === (await body.textContent()),
    "Saving a comment preserves its complete Markdown rendering",
  );
  check(
    await page
      .locator(".markdown [id]")
      .evaluateAll(
        (els) => new Set(els.map((el) => el.id)).size === els.length,
      ),
    "Repeated footnotes in descriptions and comments have unique IDs",
  );
  const hash = await page.evaluate(() => location.hash);
  const target = await comment
    .locator(".footnote-reference a")
    .getAttribute("href");
  await comment.locator(".footnote-reference a").click();
  check(
    (await page.evaluate(() => location.hash)) === hash &&
      (await comment.locator(".footnote-definition").getAttribute("id")) ===
        target.slice(1),
    "Footnote navigation stays inside its comment without changing the issue route",
  );
  check(
    await page.locator(target).evaluate((el) => {
      const rect = el.getBoundingClientRect();
      return rect.top >= 0 && rect.top < innerHeight;
    }),
    "Clicking a footnote scrolls to the correct definition",
  );

  // Attack strings are previewed only in this synthetic database.
  await page
    .locator("#comment-body")
    .fill(
      '<script>window.markdownAttack=1</script>\n\n[bad](javascript:alert(1))\n\n<img src=x onerror=alert(1)>\n\nhttps://example.com/?q=" onmouseover="alert(1)"',
    );
  await page.locator("#comment-preview").click();
  await page.waitForFunction(() =>
    document
      .querySelector("#comment-rendered")
      ?.textContent.includes("markdownAttack"),
  );
  check(
    (await page
      .locator(
        "#comment-rendered script, #comment-rendered [onerror], #comment-rendered [onmouseover], #comment-rendered a[href^='javascript:']",
      )
      .count()) === 0 && (await page.evaluate(() => !window.markdownAttack)),
    "Raw HTML, unsafe protocols and attribute injection remain inert",
  );
  await page.locator("#comment-write").click();
  await page
    .locator("#comment-body")
    .fill(
      "```text\n" +
        "long-line-".repeat(100) +
        "\n```\n\n| " +
        "Wide heading ".repeat(20) +
        " |\n| --- |\n| value |\n",
    );
  await page.locator("#comment-preview").click();
  await page.locator("#comment-rendered pre").waitFor();
  for (const width of [320, 390, 768, 1440]) {
    await page.setViewportSize({ width, height: 900 });
    check(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
      `Long code, tables and links stay within the ${width}px viewport`,
    );
  }
  await page.setViewportSize({ width: 390, height: 900 });
  const longCode = page.locator("#comment-rendered pre");
  await longCode.focus();
  await page.keyboard.press("ArrowRight");
  await page.waitForFunction(
    () => document.querySelector("#comment-rendered pre").scrollLeft > 0,
  );
  check(
    await longCode.evaluate(
      (el) => document.activeElement === el && el.tabIndex === 0,
    ),
    "Overflowing code blocks can be focused and scrolled with the keyboard",
  );
  await page.locator("#comment-write").click();
  await page.locator("#comment-body").fill("");
  // Capture the top of the reference issue in both themes and screen sizes.
  await page.goto(base + "#project=named%3AMarkdown%20QA&issue=1");
  await body.locator("h2").waitFor();
  for (const theme of ["light", "dark"]) {
    await page.emulateMedia({colorScheme: theme});
    check(
      (await body
        .locator(".token-keyword")
        .first()
        .evaluate((el) => getComputedStyle(el).color)) !==
        (await body
          .locator("pre")
          .first()
          .evaluate((el) => getComputedStyle(el).color)),
      `Syntax colors are visible in the ${theme} theme`,
    );
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.evaluate(() => scrollTo(0, 0));
    // Playwright's WebKit screenshot helper injects an inline stylesheet, which
    // this app's CSP correctly rejects. Use Chromium for visual artifacts.
    if (browserName !== "webkit")
      await page.screenshot({
        path: `output/playwright/issues-markdown-${theme}.png`,
        caret: "initial",
      });
    await page.setViewportSize({ width: 390, height: 844 });
    if (browserName !== "webkit")
      await page.screenshot({
        path: `output/playwright/issues-markdown-mobile-${theme}.png`,
        caret: "initial",
      });
  }
  check(
    errors.length === 0,
    "No JavaScript errors or content security policy violations: " +
      errors.join("; "),
  );
  return { passed: checks.length, checks, errors };
}
