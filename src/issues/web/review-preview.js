(() => {
  const dialog = $("#review-preview"), content = $("#preview-content"), submit = $("#preview-submit");
  let current;
  const clip = (text, max) => text.length > max ? text.slice(0, max - 1).trimEnd() + "…" : text;
  function block(tag, text, cls) {
    const el = document.createElement(tag);
    el.textContent = text;
    if (cls) el.className = cls;
    content.append(el);
    return el;
  }
  $("#preview-close").onclick = () => dialog.close();
  dialog.onclose = () => { current = null; submit.onclick = null; };
  window.addEventListener("hashchange", () => { if (dialog.open) dialog.close(); });
  document.addEventListener("click", async event => {
    const link = event.target.closest("a[data-review-preview]");
    if (!link || event.defaultPrevented || event.button !== 0 || event.shiftKey || event.ctrlKey || event.metaKey || event.altKey) return;
    event.preventDefault();
    const url = new URL(link.href), pr = url.pathname === "/code-review";
    const params = pr ? new URLSearchParams(url.hash.slice(1)) : url.searchParams;
    const project = params.get("project"), host = params.get("host");
    const token = current = {};
    const active = () => current === token && dialog.open;
    $("#preview-full").href = link.href;
    $("#preview-title").textContent = pr ? "Code review" : "Commit review";
    $("#preview-meta").textContent = project.replace(/^github\.com\//, "");
    content.textContent = "Loading preview…";
    submit.hidden = true;
    submit.disabled = false;
    dialog.showModal();
    let save;
    try {
      if (pr) {
        const id = Number(params.get("review"));
        const {review: r} = await api({action:"code_review",operation:{command:"view",id}}, project, null, host);
        if (!active()) return;
        $("#preview-title").textContent = r.title;
        $("#preview-meta").textContent += " · PR #" + r.number + " · " + HeyBossUI.codeReviewStage[r.stage];
        content.replaceChildren();
        block("p", clip(r.body.trim() || "No review summary.", 600));
        if (r.comments.length) {
          block("h3", "Inline feedback · " + r.comments.length);
          for (const c of r.comments.slice(0, 2)) {
            block("code", c.anchor.path + ":" + c.anchor.line);
            block("p", clip(c.body, 220), "preview-note");
          }
          if (r.comments.length > 2) block("small", "+" + (r.comments.length - 2) + " more in full review");
        }
        if (r.stage === "submitted") {
          submit.textContent = {APPROVE:"Approve and publish",COMMENT:"Publish comments",REQUEST_CHANGES:"Publish requested changes"}[r.verdict];
          save = () => post("/api/code-reviews/publish", {project,...(host ? {host} : {}),id,version:r.version,digest:r.digest});
        }
      } else {
        const number = Number(params.get("issue")), commit = params.get("commit");
        const op = operation => ({action:"commit_review",number,commit,operation});
        const r = await api(op({command:"preview"}), project, null, host);
        if (!active()) return;
        const [title,...body] = r.message.split("\n");
        $("#preview-title").textContent = clip(title, 180);
        $("#preview-meta").textContent += " · " + r.sha.slice(0, 7) + (r.approved ? " · Approved" : " · Commit");
        content.replaceChildren();
        if (body.join("\n").trim()) block("p", clip(body.join("\n").trim(), 380));
        block("h3", "Patch preview");
        const lines = r.excerpt.trimEnd().split("\n").filter(line => !line.startsWith("index "));
        const pre = block("pre", "");
        for (const line of lines.slice(0, 14)) {
          const row = document.createElement("span");
          row.className = line.startsWith("+") ? "added" : line.startsWith("-") ? "removed" : "";
          row.textContent = clip(line, 240) + "\n";
          pre.append(row);
        }
        if (!r.excerpt) pre.textContent = "No file changes.";
        if (lines.length > 14 || r.excerpt.length >= 4096) pre.append("…");
        if (!r.approved) {
          submit.textContent = "Approve commit";
          save = () => mutate(op({command:"approve",previous_message:r.message}), project, host);
        }
      }
      submit.hidden = !save;
      submit.onclick = async () => {
        if (!active() || submit.disabled || !save) return;
        submit.disabled = true;
        try {
          await save();
          if (active()) dialog.close();
          toast(pr ? "Review published to GitHub" : "Commit approved");
          await refresh(false);
        } catch (error) {
          if (active()) block("p", error.message, "form-error").setAttribute("role", "alert");
        }
      };
    } catch (error) {
      if (active()) content.textContent = error.message;
    }
  });
})();
