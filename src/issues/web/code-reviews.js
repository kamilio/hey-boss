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
  const detailPage = location.pathname === "/code-review";
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
    task,
    diff,
    serial = 0,
    busy = false,
    editing = false,
    archived = false,
    nextOffset = null,
    rows = [];
  const error = (m) => {
    const box = $("#reviews-error");
    box.hidden = !m;
    box.textContent = m || "";
  };
  const request = (operation) =>
    HeyBossUI.post(
      "/api/action",
      {
        project,
        ...(host ? { host } : {}),
        operation,
      },
      boot,
      {
        mobile: document.documentElement.dataset.issueMobile === "true",
        fallback: "Cannot open Code Reviews",
      },
    );
  const action = operation => request({action:"code_review", operation});
  const picker = new HeyBossUI.ProjectPicker({
    onSelect: (id) => {
      if (!busy && leave())
        location.href = "/code-reviews#" + new URLSearchParams({
          project: id,
          ...(host ? { host } : {}),
        });
    },
  });
  const leave = () => !editing || confirm("Discard your unsaved edit?");
  async function load(append = false) {
    const seq = ++serial;
    diff = null;
    error("");
    try {
      if (detailPage) {
        const id = Number(new URLSearchParams(location.hash.slice(1)).get("review"));
        if (!Number.isSafeInteger(id) || !id) throw Error("Missing or invalid review ID.");
        const v = await action({command: "view", id});
        if (seq !== serial) return;
        review = v.review;
        const number = Number(new URLSearchParams(location.hash.slice(1)).get("issue"));
        if (window === window.top && Number.isSafeInteger(number) && number > 0) {
          try {
            const context = await request({action:"view", number});
            if (seq !== serial) return;
            task = context;
          } catch (err) {
            if (seq !== serial) return;
            error("Cannot load task discussion: " + err.message);
          }
        }
      } else {
        const v = await action({command: "list", archived, offset: append ? nextOffset : 0});
        if (seq !== serial) return;
        nextOffset = v.next_offset;
        rows = append ? [...rows, ...v.reviews.filter(r => !rows.some(x => x.id === r.id))] : v.reviews;
      }
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
      render();
      $("#reviews-status").textContent = "Saved to Code Reviews.";
    } catch (err) {
      if (seq === serial) error(err.message);
    } finally {
      busy = false;
    }
  }
  function editor(label, body, comment, html) {
    const box = e("div", undefined, "review-editor");
    const content = e("div", body || "No review comment.", "markdown");
    if (html) content.innerHTML = html;
    box.append(content);
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
  async function showDiff(section, r, cards, fallback, url) {
    const status = e("p", "Loading changed code…", "review-empty");
    section.append(e("h2", "Changed code"), status);
    try {
      diff ??= HeyBossUI.post("/api/code-reviews/diff", {project, ...(host ? {host} : {}), id:r.id}, boot);
      const [{patch, truncated}, {parseUnifiedDiff}] = await Promise.all([diff, import("/git-review/diff.js")]);
      if (!section.isConnected) return;
      const files = parseUnifiedDiff(patch);
      status.remove();
      for (const [index, file] of files.entries()) {
        const notes = r.comments.filter(c => c.anchor.path === file.path || (c.anchor.side === "LEFT" && c.anchor.path === file.oldPath));
        const details = e("details", undefined, "review-file");
        details.append(e("summary", file.path + "  +" + file.additions + " −" + file.deletions));
        function draw() {
          if (!details.open || details.childElementCount > 1) return;
          if (!file.hunks.length) details.append(e("p", file.binary ? "Binary file" : "No text changes."));
          for (const hunk of file.hunks) {
            let pre = e("pre");
            details.append(pre);
            pre.append(e("span", hunk.header, "rv-code-line rv-hunk"));
            for (const line of hunk.lines) {
              const number = a => a.side === "LEFT" ? line.oldLine : line.newLine;
              const marked = notes.some(({anchor:a}) => number(a) !== null && number(a) >= (a.start_line || a.line) && number(a) <= a.line);
              const row = e("span", undefined, "rv-code-line " + line.type + (marked ? " marked" : ""));
              row.append(e("span", line.oldLine ?? "", "rv-line-number"), e("span", line.newLine ?? "", "rv-line-number"),
                document.createTextNode(({addition:"+", deletion:"-", context:" "})[line.type] + line.content));
              pre.append(row);
              for (const c of notes.filter(c => number(c.anchor) === c.anchor.line)) {
                const card = cards.get(c.id);
                if (card.contains(document.activeElement)) continue;
                card.querySelector("pre")?.remove();
                details.append(card);
                pre = e("pre");
                details.append(pre);
              }
            }
            if (!pre.childNodes.length) pre.remove();
          }
        }
        details.open = index === 0 || notes.length > 0;
        details.ontoggle = draw;
        section.append(details);
        draw();
      }
      if (!r.comments.some(c => cards.get(c.id).parentNode === fallback)) fallback.hidden = true;
      if (truncated || !files.length) {
        status.textContent = truncated ? "Large diff: showing the first part. " : "No changed code to display. ";
        status.append(link("View files on GitHub ↗", url + "/files"));
        section.append(status);
      }
    } catch (err) {
      status.textContent = err.message + " ";
      status.append(link("View files on GitHub ↗", url + "/files"));
    }
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
        const dialog = window.frameElement?.closest("#review-preview");
        if (r.verdict === "APPROVE" && dialog?.open) {
          dialog.close();
          return;
        }
        await load();
        $("#reviews-status").textContent = "Published to GitHub.";
      } catch (err) {
        if (seq === serial) {
          await load();
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
    const list = $("#reviews-list");
    list.setAttribute("aria-label", detailPage ? "Code review" : "Review queue");
    list.replaceChildren();
    list.classList.toggle("merge-rows", !detailPage && rows.length > 0);
    $(".review-views").hidden = detailPage;
    $("#reviews-count").textContent = detailPage ? "" : rows.length + (nextOffset != null ? "+" : "");
    $("#reviews-more").hidden = detailPage || nextOffset == null;
    const title = review?.title || (detailPage ? "Code Review" : "Code Reviews");
    $("#reviews-title").textContent = title;
    document.title = title + " · Hey Boss";
    const hash = new URLSearchParams(location.hash.slice(1));
    hash.delete("review");
    hash.delete("issue");
    const back = $("#reviews-back");
    back.hidden = !detailPage;
    back.href = "/code-reviews#" + hash;
    if (!detailPage) {
      $("#reviews-pending").setAttribute("aria-pressed", String(!archived));
      $("#reviews-archive").setAttribute("aria-pressed", String(archived));
      for (const r of rows) {
        hash.set("review", r.id);
        const row = e("div", undefined, "review-index-row");
        const summary = e("a", undefined, "merge-row review-row");
        summary.href = "/code-review#" + hash;
        const icon = e("span", undefined, "review-pr-icon");
        icon.innerHTML = HeyBossUI.icon("pull-request");
        const main = e("span", undefined, "merge-main");
        const meta = e("span", "#" + r.number + " · " + stage[r.stage], "merge-meta");
        if (r.comment_count) meta.append(" · " + r.comment_count + " inline comment" + (r.comment_count === 1 ? "" : "s"));
        main.append(e("strong", r.title, "merge-title"), meta);
        if (r.body.trim()) main.append(e("span", r.body, "review-preview"));
        if (r.first_comment) main.append(e("span", "Inline: " + r.first_comment, "review-preview"));
        summary.append(icon, main, e("span", undefined, "review-chevron"));
        row.append(summary);
        if (r.stage === "submitted") {
          const publish = publishButton(r);
          publish.classList.add("small", "review-quick-publish");
          row.append(publish);
        }
        list.append(row);
      }
      if (!rows.length)
        list.append(e("p", archived ? "No archived reviews." : "No pending reviews.", "review-empty"));
      return;
    }
    if (!review) return;
    const r = review;
    const pane = e("section");
    pane.id = "reviews-detail";
    pane.setAttribute("aria-label", "Review details");
    list.append(pane);
    const repo = /^github\.com\/[\w.-]+\/[\w.-]+$/i.test(project) && project;
    const url = repo ? "https://" + repo + "/pull/" + r.number : null;

    const meta = e("div", undefined, "review-meta");
    meta.append(
      e("span", "#" + r.number + " · " + stage[r.stage]),
      e("strong", "Verdict: " + verdict[r.verdict]),
      e("code", r.head.slice(0, 8)),
    );
    if (url) meta.append(link("Open PR #" + r.number + " ↗", url));
    pane.append(
      meta,
      e("h2", "Review comment"),
      editor("review comment", r.body, null, r.body_html),
    );
    const changes = e("section"), fallback = e("section"), cards = new Map();
    pane.append(changes, fallback);
    fallback.append(e("h2", "Inline comments · " + r.comments.length));
    if (!r.comments.length) fallback.append(e("p", "No inline comments."));
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
        editor("comment on " + a.path + " line " + a.line, c.body, c.id, c.body_html),
      );
      fallback.append(card);
      cards.set(c.id, card);
    }
    if (url && ["submitted", "draft"].includes(r.stage)) showDiff(changes, r, cards, fallback, url);
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
          await load();
          $("#reviews-status").textContent = "Review discarded.";
        } catch (err) {
          if (seq === serial) error(err.message);
        } finally { busy = false; }
      });
      discard.classList.add("review-discard");
      footer.append(discard);
    }
    pane.insertBefore(footer, changes);
    if (task) pane.append(HeyBossUI.reviewDiscussion(task, project, host));
  }
  async function navigate() {
    if (!detailPage && new URLSearchParams(location.hash.slice(1)).has("review")) {
      location.replace("/code-review" + location.hash);
      return;
    }
    serial++;
    review = task = null;
    rows = [];
    archived = new URLSearchParams(location.hash.slice(1)).get("view") === "archive";
    nextOffset = null;
    $("#reviews-status").textContent = "";
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
    await load();
  }
  for (const [id, archive] of [["#reviews-pending", false], ["#reviews-archive", true]]) {
    $(id).onclick = () => {
      if (busy || archived === archive || !leave()) return;
      const hash = new URLSearchParams(location.hash.slice(1));
      hash.delete("review");
      if (archive) hash.set("view", "archive");
      else hash.delete("view");
      location.hash = hash;
    };
  }
  $("#reviews-more").onclick = () => {
    if (!busy && leave()) load(true);
  };
  $("#reviews-refresh").onclick = () => {
    if (!busy && leave()) load();
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
