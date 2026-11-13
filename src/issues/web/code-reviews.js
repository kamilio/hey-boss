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
  const stage = HeyBossUI.codeReviewStage;
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
      const target = rows.find((x) => x.id == id);
      let detail = null;
      if (id) {
        detail = (await action({ command: "view", id: Number(id) })).review;
        if (!target) rows = [detail, ...rows];
      }
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
    box.append(e("p", body || "No review comment."));
    if (!["draft", "submitted"].includes(review.stage)) return box;
    const controls = e("div", undefined, "review-controls");
    const edit = btn("Edit", () => {
      if (busy || editing) return;
      editing = true;
      const ta = e("textarea", undefined, "text-input");
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
      save.classList.add("small", "primary");
      cancel.classList.add("small");
      controls.replaceChildren(save, cancel);
      box.replaceChildren(ta, controls);
      ta.focus();
    });
    edit.setAttribute("aria-label", "Edit " + label);
    edit.className = "icon-button";
    edit.title = "Edit " + label;
    edit.innerHTML = HeyBossUI.icon("edit");
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
  function publishButton(r) {
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
        const current = r.digest ? r : (await action({ command: "view", id: r.id })).review;
        if (seq !== serial) return;
        if (current.version !== r.version || current.stage !== "submitted")
          throw Error("This review changed. Refresh the queue before publishing.");
        await HeyBossUI.post(
          "/api/code-reviews/publish",
          {
            project,
            ...(host ? { host } : {}),
            id: r.id,
            version: r.version,
            digest: current.digest,
          },
          boot,
          { fallback: "Publication failed" },
        );
        if (seq !== serial) return;
        await load(review?.id);
        $("#reviews-status").textContent = "Published to GitHub.";
      } catch (err) {
        if (seq === serial) {
          await load(review?.id);
          error(err.message);
        }
      } finally {
        busy = false;
        publish.disabled = false;
      }
    });
    publish.classList.add("primary");
    return publish;
  }
  function render() {
    const list = $("#reviews-list"),
      pane = e("section");
    pane.id = "reviews-detail";
    pane.setAttribute("aria-label", "Selected review");
    list.replaceChildren();
    list.classList.toggle("merge-rows", rows.length > 0);
    $("#reviews-count").textContent = rows.length;
    const repo = /^github\.com\/[\w.-]+\/[\w.-]+$/i.test(project) && project;
    for (const r of rows) {
      const selected = r.id === review?.id;
      const b = btn("", async () => {
        if (!busy && leave()) {
          const hash = new URLSearchParams(location.hash.slice(1));
          if (selected) hash.delete("review");
          else hash.set("review", r.id);
          history.replaceState(null, "", "#" + hash);
          if (selected) {
            ++serial;
            review = null;
            editing = false;
            render();
          } else await load(r.id);
          list.querySelector('[data-review-id="' + r.id + '"]')
            ?.focus({ preventScroll: true });
        }
      });
      b.className = "review-toggle";
      b.setAttribute("aria-label", (selected ? "Collapse" : "Expand") + " review for PR #" + r.number);
      b.dataset.reviewId = r.id;
      b.setAttribute("aria-expanded", String(selected));
      const icon = e("span", undefined, "review-pr-icon");
      icon.innerHTML = HeyBossUI.icon("pull-request");
      const main = e("span", undefined, "merge-main");
      const title = repo ? link(r.title, "https://" + repo + "/pull/" + r.number) : e("strong", r.title);
      title.className = "merge-title";
      main.append(
        title,
        e("span", "#" + r.number + " · " + stage[r.stage], "merge-meta"),
      );
      const count = r.comments?.length ?? r.comment_count;
      if (!selected) {
        const body = r.body.trim();
        const inline = r.comments?.[0]?.body || r.first_comment;
        if (body) main.append(e("span", body, "review-preview"));
        if (inline) main.append(e("span", "Inline: " + inline, "review-preview"));
        if (count) main.querySelector(".merge-meta").append(" · " + count + " inline comment" + (count === 1 ? "" : "s"));
      }
      const arrow = e("span", undefined, "review-chevron");
      arrow.setAttribute("aria-hidden", "true");
      b.append(arrow);
      const row = e("div", undefined, "review-index-row");
      const summary = e("div", undefined, "merge-row review-row");
      summary.append(icon, main, b);
      row.append(summary);
      if (!selected && r.stage === "submitted") {
        const publish = publishButton(r);
        publish.classList.add("small", "review-quick-publish");
        row.append(publish);
      }
      list.append(row);
      if (selected) list.append(pane);
    }
    if (!review) {
      if (!rows.length)
        list.append(
          e("p", "No reviews yet. Submitted PR reviews for this project will appear here.", "review-empty"),
        );
      return;
    }
    const r = review;
    const url = repo ? "https://" + repo + "/pull/" + r.number : null;

    const meta = e("div", undefined, "review-meta");
    meta.append(
      e("strong", "Verdict: " + verdict[r.verdict]),
      e("code", r.head.slice(0, 8)),
    );
    if (url) meta.append(link("Open PR #" + r.number + " ↗", url));
    pane.append(
      meta,
      e("h2", "Review comment"),
      editor("review comment", r.body, null),
      e("h2", "Inline comments · " + r.comments.length),
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
          anchor = link("", url + "/files");
        const parts = a.path.split("/");
        parts.forEach((part, i) => {
          if (i) anchor.append("/", e("wbr"));
          anchor.append(part);
        });
        anchor.append(header.textContent.slice(a.path.length));
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
      footer.append(
        publishButton(r),
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
    if (r.stage === "outdated")
      footer.append(e("small", "Review the new commit before publishing."));
    if (["draft", "submitted", "outdated"].includes(r.stage)) {
      const discard = btn("Discard review", async () => {
        if (busy || !confirm("Discard this review? It will be removed from the queue without publishing.")) return;
        busy = true;
        const seq = serial;
        try {
          await action({ command: "discard", id: r.id, version: r.version });
          if (seq !== serial) return;
          const hash = new URLSearchParams(location.hash.slice(1));
          hash.delete("review");
          history.replaceState(null, "", "#" + hash);
          await load();
          $("#reviews-status").textContent = "Review discarded.";
        } catch (err) {
          if (seq === serial) error(err.message);
        } finally { busy = false; }
      });
      discard.classList.add("review-discard");
      footer.append(discard);
    }
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
