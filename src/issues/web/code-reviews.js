"use strict";
(async () => {
  const $ = (s) => document.querySelector(s),
    e = (tag, text, cls) => {
      const el = document.createElement(tag);
      if (text !== undefined) el.textContent = text;
      if (cls) el.className = cls;
      return el;
    };
  const btn = (label, fn) => {
    const b = e("button", label, "button");
    b.type = "button";
    b.onclick = fn;
    return b;
  };
  const link = (text, url) => {
    const a = e("a", text);
    a.href = url;
    a.target = "_blank";
    a.rel = "noopener noreferrer";
    return a;
  };
  const stage = {
    draft: "Draft",
    submitted: "Awaiting approval",
    publishing: "Publication pending",
    published: "Published",
    outdated: "Outdated",
  };
  const verdict = {
    COMMENT: "Comment",
    APPROVE: "Approve",
    REQUEST_CHANGES: "Request changes",
  };
  let boot,
    project,
    host,
    review,
    serial = 0,
    busy = false,
    editing = false,
    rows = [];
  const error = (m) => {
    const box = $("#reviews-error");
    box.hidden = !m;
    box.textContent = m || "";
  };
  const action = (op) =>
    HeyBossUI.post(
      "/api/action",
      {
        project,
        ...(host ? { host } : {}),
        operation: { action: "code_review", operation: op },
      },
      boot,
      {
        mobile: document.documentElement.dataset.issueMobile === "true",
        fallback: "Cannot open Code Reviews",
      },
    );
  const picker = new HeyBossUI.ProjectPicker({
    onSelect: (id) => {
      if (!busy && leave())
        location.hash = new URLSearchParams({
          project: id,
          ...(host ? { host } : {}),
        });
    },
  });
  const leave = () => !editing || confirm("Discard your unsaved edit?");
  async function load(id) {
    const seq = ++serial;
    error("");
    try {
      const v = await action({ command: "list" });
      if (seq !== serial) return;
      rows = v.reviews;
      const target = rows.find((x) => x.id == id) || rows[0];
      let detail = null;
      if (target)
        detail = (await action({ command: "view", id: target.id })).review;
      if (seq !== serial) return;
      review = detail;
      editing = false;
      render();
    } catch (err) {
      if (seq === serial) error(err.message);
    }
  }
  async function mutate(op) {
    if (busy) return;
    busy = true;
    const seq = serial;
    try {
      const v = await action(op);
      if (seq !== serial) return;
      review = v.review;
      editing = false;
      rows = rows.map((x) => (x.id === review.id ? review : x));
      render();
      $("#reviews-status").textContent = "Saved to Code Reviews.";
    } catch (err) {
      if (seq === serial) error(err.message);
    } finally {
      busy = false;
    }
  }
  function editor(label, body, comment) {
    const box = e("div", undefined, "review-editor");
    box.append(e("p", body || "No summary."));
    if (!["draft", "submitted"].includes(review.stage)) return box;
    const controls = e("div", undefined, "review-controls");
    const edit = btn("Edit", () => {
      if (busy || editing) return;
      editing = true;
      const ta = e("textarea");
      ta.value = body;
      ta.setAttribute("aria-label", label);
      const save = btn("Save", () =>
        mutate({
          command: "edit",
          id: review.id,
          version: review.version,
          comment,
          body: ta.value,
        }),
      );
      const cancel = btn("Cancel", () => {
        if (busy) return;
        editing = false;
        render();
      });
      controls.replaceChildren(save, cancel);
      box.replaceChildren(ta, controls);
      ta.focus();
    });
    edit.setAttribute("aria-label", "Edit " + label);
    controls.append(edit);
    box.append(controls);
    return box;
  }
  function excerpt(c) {
    const pre = e("pre"),
      lines = c.snippet.split("\n"),
      h = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(lines[0]);
    if (!h) {
      pre.textContent = c.snippet;
      return pre;
    }
    let left = Number(h[1]),
      right = Number(h[2]);
    for (const line of lines.slice(1)) {
      const old = line[0] !== "+" ? left++ : null,
        now = line[0] !== "-" ? right++ : null,
        n = c.anchor.side === "LEFT" ? old : now,
        marked =
          n !== null &&
          n >= (c.anchor.start_line || c.anchor.line) &&
          n <= c.anchor.line;
      const row = e(
        "span",
        undefined,
        "rv-code-line" + (marked ? " marked" : ""),
      );
      row.append(
        e("span", n ?? "", "rv-line-number"),
        document.createTextNode(line),
      );
      pre.append(row);
    }
    return pre;
  }
  function render() {
    const list = $("#reviews-list"),
      pane = $("#reviews-detail");
    list.replaceChildren();
    pane.replaceChildren();
    $("#reviews-count").textContent = rows.length;
    for (const r of rows) {
      const b = btn("", () => {
        if (!busy && leave()) {
          const hash = new URLSearchParams(location.hash.slice(1));
          hash.set("review", r.id);
          history.replaceState(null, "", "#" + hash);
          load(r.id);
        }
      });
      b.setAttribute("aria-current", String(r.id === review?.id));
      b.append(
        e("small", "#" + r.number + " · " + stage[r.stage]),
        e("strong", r.title),
      );
      list.append(b);
    }
    if (!review) {
      pane.append(
        e(
          "p",
          "No reviews yet. Submitted PR reviews for this project will appear here.",
          "review-empty",
        ),
      );
      return;
    }
    const r = review;
    const repo = project.match(/^github\.com\/([\w.-]+\/[\w.-]+)$/i)?.[1];
    const url = repo
      ? "https://github.com/" + repo + "/pull/" + r.number
      : null;
    pane.append(
      e("div", stage[r.stage].toUpperCase(), "review-kicker"),
      e("h2", r.title),
    );
    const meta = e("div", undefined, "review-meta");
    meta.append(
      e("span", verdict[r.verdict], "review-chip"),
      e("code", r.head.slice(0, 8)),
    );
    if (url) meta.append(link("Open PR #" + r.number + " ↗", url));
    pane.append(
      meta,
      e("h3", "Review summary"),
      editor("review summary", r.body, null),
      e("h3", "Inline comments · " + r.comments.length),
    );
    if (!r.comments.length) pane.append(e("p", "No inline comments."));
    for (const c of r.comments) {
      const a = c.anchor,
        card = e("article", undefined, "review-comment");
      card.append(
        e(
          "header",
          a.path +
            ":" +
            (a.start_line ? a.start_line + "–" : "") +
            a.line +
            " · " +
            (a.side === "LEFT" ? "before" : "after"),
        ),
      );
      if (url) {
        const header = card.firstChild,
          anchor = link(header.textContent, url + "/files");
        header.replaceChildren(anchor);
        crypto.subtle
          ?.digest("SHA-256", new TextEncoder().encode(a.path))
          .then((buf) => {
            anchor.href =
              url +
              "/files#diff-" +
              Array.from(new Uint8Array(buf), (b) =>
                b.toString(16).padStart(2, "0"),
              ).join("") +
              (a.side === "LEFT" ? "L" : "R") +
              a.line;
          });
      }
      if (c.snippet) card.append(excerpt(c));
      card.append(
        editor("comment on " + a.path + " line " + a.line, c.body, c.id),
      );
      pane.append(card);
    }
    const footer = e("div", undefined, "review-actions");
    if (r.stage === "submitted") {
      const label = {
        APPROVE: "Approve and publish",
        COMMENT: "Publish comments",
        REQUEST_CHANGES: "Publish requested changes",
      }[r.verdict];
      const publish = btn(label, async () => {
        if (busy || editing) {
          error("Save or cancel your edit before publishing.");
          return;
        }
        busy = true;
        const seq = serial;
        publish.disabled = true;
        error("");
        try {
          await HeyBossUI.post(
            "/api/code-reviews/publish",
            {
              project,
              ...(host ? { host } : {}),
              id: r.id,
              version: r.version,
              digest: r.digest,
            },
            boot,
            { fallback: "Publication failed" },
          );
          if (seq !== serial) return;
          await load(r.id);
          $("#reviews-status").textContent = "Published to GitHub.";
        } catch (err) {
          if (seq === serial) error(err.message);
        } finally {
          busy = false;
          publish.disabled = false;
        }
      });
      publish.classList.add("publish");
      footer.append(
        publish,
        e("small", "Publishes under your GitHub account."),
      );
    } else if (
      r.stage === "published" &&
      url &&
      r.receipt.startsWith(url + "#")
    )
      footer.append(
        link("View published review ↗", r.receipt),
        e("small", "Published by " + r.published_by),
      );
    else if (r.stage === "publishing")
      footer.append(
        e("p", "Check GitHub for the publication receipt before retrying."),
      );
    pane.append(footer);
  }
  async function navigate() {
    serial++;
    review = null;
    rows = [];
    editing = false;
    render();
    host =
      document.documentElement.dataset.issueMobile === "true"
        ? null
        : new URLSearchParams(location.hash.slice(1)).get("host") ||
          boot.backend_host ||
          null;
    project = HeyBossUI.projectId(boot.project?.id || boot.projects[0]?.id);
    const p = boot.projects.find((x) => x.id === project);
    picker.update(boot.projects, p);
    $("#nav-code-reviews")?.setAttribute("aria-current", "page");
    if (!p?.code_reviews_enabled) {
      error("Code Reviews is disabled for this project.");
      return;
    }
    await load(new URLSearchParams(location.hash.slice(1)).get("review"));
  }
  $("#reviews-refresh").onclick = () => {
    if (!busy && leave()) load(review?.id);
  };
  window.addEventListener("beforeunload", (ev) => {
    if (editing) {
      ev.preventDefault();
      ev.returnValue = "";
    }
  });
  window.addEventListener("hashchange", () => boot && navigate());
  try {
    const res = await fetch("/api/bootstrap");
    boot = await res.json();
    if (!res.ok || !boot.ok) throw Error("Cannot load projects");
    HeyBossUI.icons();
    await navigate();
  } catch (err) {
    error(err.message);
  }
})();
