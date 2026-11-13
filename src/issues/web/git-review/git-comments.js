import { createGitActivity } from "./git-activity.js";


export function mountGitComments({ legacy = false, comments: initialComments = [], createActivity = createGitActivity, request } = {}) {
  const root = document.querySelector("#files") ?? document.querySelector("main"), count = document.querySelector("#count");
  const escape = (value) => String(value ?? "").replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character]);
  const pointOf = (comment) => comment.location ?? comment;
  const groupKey = (point) => JSON.stringify([point.file, point.side, point.commit ?? "", point.section ?? ""]);
  const label = (point) => `${point.file}:${point.line}${point.endLine && point.endLine !== point.line ? `–${point.endLine}` : ""} · ${point.side} side`;
  const groups = new Map(), rowEntries = new WeakMap(), editors = new Map(), deleting = new Map(), pendingIds = new Set();
  const activityFiles = new WeakMap(), fileComments = new Map();
  const activity = (owner, state, action = "save") => {
    const file = activityFiles.get(owner) ?? owner.closest(".file,.outside-comments"); if (!file) return;
    activityFiles.set(owner, file); file.dispatchEvent(new CustomEvent("git-comment-activity", { bubbles: true, detail: { owner, state, action } }));
  };
  const entries = [...root.querySelectorAll(".file")].flatMap((file) => {
    fileComments.set(file, { count: 0, ids: new Set(), targets: [] });
    const context = JSON.parse(file.dataset.reviewContext ?? "{}");
    return [...file.querySelectorAll(legacy ? ".row[data-file]" : ".diff-row[data-file]")].map((row) => {
      const primary = { ...context, file: row.dataset.file, side: row.dataset.side, line: Number(row.dataset.line) };
      const variant = row.closest(".split-view,.split-hunk") ? "split" : "unified";
      const old = row.classList.contains("context") ? { ...primary, side: "old", line: Number(row.dataset.oldLine) } : null;
      const point = old && row.closest('[data-diff-side="Old"]') ? old : primary;
      const points = old && variant === "unified" ? [primary, old] : [point];
      const entry = { file, row, point, points, button: row.querySelector(".add-comment"), views: new Map(), thread: null, variant };
      row.dataset.side = point.side; row.dataset.line = point.line;
      entry.button.setAttribute("aria-label", `Comment on ${point.file} ${point.side} line ${point.line}`); entry.button.title = label(point);
      for (const location of points) { const key = groupKey(location); if (!groups.has(key)) groups.set(key, []); groups.get(key).push({ entry, point: location }); }
      rowEntries.set(row, entry); return entry;
    });
  });
  for (const group of groups.values()) group.sort((first, second) => first.point.line - second.point.line);
  const lineStart = (group, line) => {
    let lower = 0, upper = group.length;
    while (lower < upper) { const middle = (lower + upper) >>> 1; if (group[middle].point.line < line) lower = middle + 1; else upper = middle; }
    return lower;
  };
  const destinationFor = (point, variant) => {
    const group = groups.get(groupKey(point)) ?? [], line = Number(point.endLine ?? point.line);
    for (let index = lineStart(group, line); index < group.length && group[index].point.line === line; index++) if (group[index].entry.variant === variant) return group[index].entry;
  };
  let comments = initialComments, rangeAnchor = null, pendingWrites = 0, loading = false;
  const loadStatus = document.createElement("div"); loadStatus.className = "comment-load-status"; loadStatus.hidden = true;
  loadStatus.innerHTML = '<span role="status" aria-live="polite"></span><button class="retry-comments" type="button">Retry comments</button>';
  root.prepend(loadStatus);
  const loadMessage = loadStatus.querySelector("span"), retry = loadStatus.querySelector("button");
  const outside = document.createElement("details"); outside.className = "outside-comments"; outside.id = "git-outside-comments"; outside.hidden = true; outside.open = true;
  outside.innerHTML = '<summary>Comments outside this diff<span class="outside-count" aria-live="polite"></span></summary><p class="outside-help">These saved locations are not fully shown in this diff. You can edit the notes or delete them; their original locations stay unchanged.</p>';
  root.prepend(outside);
  const outsideCount = outside.querySelector(".outside-count"), outsideEntry = { row: outside.querySelector(".outside-help"), points: [], button: outside.querySelector("summary"), views: new Map(), thread: null, variant: "outside" };
  const outsideFocus = () => outside.hidden ? document.querySelector('.git-layout [aria-pressed="true"]') : outsideEntry.button;
  const outsideActivity = createActivity({ name: "comments outside this diff", className: "git-outside-activity", controls: outside.id, prefix: "Outside-diff comments: ", insert: notice => outside.before(notice), collapsed: () => !outside.open, reveal: () => { if (!outside.hidden) outside.open = true; }, focus: outsideFocus, completedAction: () => outside.hidden ? "Back to review" : "Show comments" });
  outside.addEventListener("toggle", () => {
    if (outside.open) outsideActivity.acknowledge();
    else if (outside.contains(document.activeElement) && document.activeElement !== outsideEntry.button) outsideEntry.button.focus({ preventScroll: true });
  });
  root.addEventListener("git-comment-activity", event => { if (event.target === outside) { const { owner, state, action } = event.detail; outsideActivity.update(owner, state, action); } });
  root.addEventListener("click", event => { if (event.target.closest(".git-outside-activity button")) outsideActivity.show(); });
  entries.push(outsideEntry);
  const entryLabel = (entry, point) => label(point) + (entry === outsideEntry ? `${point.commit ? ` · commit ${point.commit}` : ""}${point.section ? ` · ${point.section} section` : ""}` : "");
  const focusEntry = (entry, id) => {
    const target = id ? entry.views.get(id)?.querySelector(".edit-comment") : null;
    (target ?? (entry === outsideEntry && outside.hidden ? document.querySelector('.git-layout [aria-pressed="true"]') : entry.button))?.focus();
  };
  const syncRetry = () => { retry.disabled = loading || pendingWrites > 0; retry.textContent = loading ? "Loading…" : "Retry comments"; retry.setAttribute("aria-busy", String(loading)); };
  const api = async (suffix, options = {}) => {
    if (request) {
      const id = decodeURIComponent(suffix.slice(1)), saved = comments.find(c => c.id === id);
      const input = options.body ? JSON.parse(options.body) : {};
      const command = options.method === 'POST' ? {command:'comment', ...input} : options.method === 'PUT' ? {command:'edit_comment',id,...input,previous_body:saved?.body} : options.method === 'DELETE' ? {command:'delete_comment',id,previous_body:saved?.body} : {command:'comments'};
      const result = await request(command);
      return result.comment ?? result.comments ?? result;
    }
    const response = await fetch(location.pathname + "/comments" + suffix, { cache: "no-store", ...options }), text = await response.text();
    let result; try { result = JSON.parse(text); } catch { throw new Error(text || "The review server returned an invalid response."); }
    if (!response.ok) throw new Error(result.error || `Review server error ${response.status}.`);
    return result;
  };
  const threadFor = (entry) => {
    if (!entry.thread) { entry.thread = document.createElement("div"); entry.thread.className = "comment-thread"; entry.row.after(entry.thread); }
    return entry.thread;
  };
  const trimThread = (entry) => { if (entry.thread && !entry.thread.children.length) { entry.thread.remove(); entry.thread = null; } };
  const toolsHtml = '<button type="button" class="edit-comment edit">Edit</button><button type="button" class="delete-comment delete">Delete</button>';
  const editorPoint = (form) => JSON.parse(form.dataset.location);
  const formEntry = new WeakMap();
  root.addEventListener("git-show-file-comments", event => {
    const record = fileComments.get(event.target), variant = document.body.classList.contains("split") ? "split" : "unified";
    const entry = record?.targets.find(candidate => candidate.variant === variant); if (!entry) return;
    const id = entry.views.keys().next().value, form = editors.get(id);
    const target = form && formEntry.get(form) === entry ? form.querySelector("textarea") : entry.views.get(id);
    target?.focus();
  });
  function draw() {
    if (count) count.textContent = `${comments.length} comment${comments.length === 1 ? "" : "s"}`;
    for (const record of fileComments.values()) { record.ids.clear(); record.targets.length = 0; }
    const covering = new Map(), endpoints = new Map(), missing = [];
    for (const comment of comments) {
      const point = pointOf(comment), group = groups.get(groupKey(point)) ?? [], first = Number(point.line), end = Number(point.endLine ?? point.line);
      if (!Number.isSafeInteger(first) || !Number.isSafeInteger(end) || first < 1 || end < first) { missing.push(comment); continue; }
      const start = lineStart(group, first); let stop = start, covered = 0, previous;
      while (stop < group.length && group[stop].point.line <= end) { const line = group[stop].point.line; if (line !== previous) covered++; previous = line; stop++; }
      if (!covered || covered !== end - first + 1) { missing.push(comment); continue; }
      for (let index = start; index < stop; index++) {
        const { entry, point: location } = group[index]; covering.set(entry, (covering.get(entry) ?? 0) + 1);
        if (location.line === end) { if (!endpoints.has(entry)) endpoints.set(entry, []); endpoints.get(entry).push(comment); }
      }
    }
    outside.hidden = missing.length === 0; outsideActivity.refresh();
    if (outside.hidden && outside.contains(document.activeElement)) (outsideActivity.control?.getClientRects().length ? outsideActivity.control : outsideFocus())?.focus({ preventScroll: true });
    const missingLabel = `(${missing.length})`; if (outsideCount.textContent !== missingLabel) outsideCount.textContent = missingLabel;
    endpoints.set(outsideEntry, missing);
    for (const entry of entries) {
      const total = covering.get(entry) ?? 0;
      if (entry !== outsideEntry && entry.count !== total) { entry.button.classList.toggle("has-comments", total > 0); entry.button.textContent = total || "+"; entry.count = total; }
      const found = endpoints.get(entry) ?? [], ids = new Set(found.map((comment) => comment.id));
      const record = fileComments.get(entry.file);
      if (record && found.length) { record.targets.push(entry); for (const id of ids) record.ids.add(id); }
      for (const [id, view] of entry.views) if (!ids.has(id)) { view.remove(); entry.views.delete(id); }
      for (const comment of found) {
        let view = entry.views.get(comment.id);
        if (!view) { view = document.createElement("div"); view.className = "comment-view comment"; view.dataset.id = comment.id; view.dataset.commentId = comment.id; view.tabIndex = -1; view.setAttribute("role", "group"); view.setAttribute("aria-label", `Saved comment on ${entryLabel(entry, pointOf(comment))}`); entry.views.set(comment.id, view); threadFor(entry).append(view); }
        const signature = JSON.stringify([comment.body, comment.updatedAt]);
        if (view.dataset.signature !== signature && !deleting.has(comment.id)) {
          view.innerHTML = `<div class="comment-body">${escape(comment.body)}</div><small class="comment-meta"><span>${escape(new Date(comment.createdAt).toLocaleString())} · ${escape(entryLabel(entry, pointOf(comment)))}${comment.updatedAt !== comment.createdAt ? " · edited" : ""}</span><span class="comment-tools tools">${toolsHtml}</span></small><div class="comment-error form-error" role="alert"></div>`;
          view.dataset.signature = signature;
        }
        const hidden = editors.has(comment.id) && formEntry.get(editors.get(comment.id)) === entry;
        if (view.hidden !== hidden) view.hidden = hidden;
        for (const button of view.querySelectorAll(".edit-comment,.delete-comment")) if (button.disabled !== pendingIds.has(comment.id)) button.disabled = pendingIds.has(comment.id);
      }
      trimThread(entry);
    }
    for (const [file, record] of fileComments) if (record.count !== record.ids.size) {
      record.count = record.ids.size; file.dispatchEvent(new CustomEvent("git-file-comment-count", { bubbles: true, detail: { count: record.count } }));
    }
  }
  function closeForm(form, restoreFocus, activityState = "clear") {
    const entry = formEntry.get(form), id = form.dataset.id;
    activity(form, activityState);
    if (id) editors.delete(id); form.remove();
    if (rangeAnchor?.form === form) rangeAnchor = null;
    for (const candidate of entries) candidate.row.classList.remove("range-selected");
    draw(); trimThread(entry);
    if (restoreFocus) focusEntry(entry, id);
  }
  function openForm(entry, point, comment) {
    let form = comment ? editors.get(comment.id) : [...(entry.thread?.querySelectorAll('.comment-form:not([data-id])') ?? [])].find((node) => JSON.stringify(editorPoint(node)) === JSON.stringify(point));
    if (form) {
      if (form.dataset.pending === "true") return form;
      const previous = formEntry.get(form);
      if (previous !== entry) { threadFor(entry).append(form); formEntry.set(form, entry); trimThread(previous); draw(); }
      form.querySelector("textarea").focus(); return form;
    }
    form = document.createElement("form"); form.className = "comment-form"; form.dataset.location = JSON.stringify(point);
    if (comment) { form.dataset.id = comment.id; form.dataset.commentId = comment.id; editors.set(comment.id, form); }
    form.innerHTML = `<div class="comment-location">${escape(entryLabel(entry, point))} · ${entry === outsideEntry ? "Original location kept" : "Shift-click another + to select a range"}</div><textarea required maxlength="10000" aria-label="Comment text" placeholder="Leave a clear, actionable comment…">${escape(comment?.body ?? "")}</textarea><div class="comment-actions actions"><span class="comment-error form-error" role="alert"></span><span class="comment-shortcut">Ctrl+Enter to save</span><button type="button" class="cancel-comment cancel">Cancel</button><button class="submit-comment save-comment" aria-busy="false">${comment ? "Save changes" : "Add comment"}</button></div>`;
    formEntry.set(form, entry);
    if (comment && entry === outsideEntry) entry.views.get(comment.id).after(form); else threadFor(entry).append(form);
    draw(); form.querySelector("textarea").focus(); return form;
  }
  document.addEventListener("click", (event) => {
    if (!event.target.closest("[data-layout]")) return;
    const variant = document.body.classList.contains("split") ? "split" : "unified", focused = document.activeElement;
    const active = entries.filter((entry) => entry !== outsideEntry && entry.variant !== variant && entry.thread && (entry.thread.querySelector(".comment-form") || [...entry.views.values()].some((view) => deleting.get(view.dataset.id) === view)));
    const moveView = (entry, destination, id) => {
      const view = entry.views.get(id); if (!view) return;
      const counterpart = destination.views.get(id);
      if (counterpart) {
        const marker = document.createTextNode(""); view.replaceWith(marker); counterpart.replaceWith(view); marker.replaceWith(counterpart); entry.views.set(id, counterpart);
      } else { threadFor(destination).append(view); entry.views.delete(id); }
      destination.views.set(id, view);
    };
    let moved = false;
    for (const entry of active) {
      for (const form of [...entry.thread.querySelectorAll(".comment-form")]) {
        const destination = destinationFor(editorPoint(form), variant); if (!destination) continue;
        threadFor(destination).append(form); formEntry.set(form, destination);
        if (form.dataset.id) moveView(entry, destination, form.dataset.id);
        moved = true;
      }
      for (const [id, view] of [...entry.views]) {
        if (deleting.get(id) !== view) continue;
        const comment = comments.find((candidate) => candidate.id === id), destination = comment && destinationFor(pointOf(comment), variant); if (!destination) continue;
        moveView(entry, destination, id); moved = true;
      }
      trimThread(entry);
    }
    if (moved && focused.isConnected && focused !== document.body && document.activeElement !== focused && focused.getClientRects().length) focused.focus({ preventScroll: true });
  });
  const busyForm = (form, pending) => {
    form.dataset.pending = String(pending); form.setAttribute("aria-busy", String(pending)); form.querySelector("textarea").readOnly = pending;
    for (const button of form.querySelectorAll("button")) button.disabled = pending;
    const button = form.querySelector(".submit-comment"); button.textContent = pending ? "Saving…" : form.dataset.id ? "Save changes" : "Add comment"; button.setAttribute("aria-busy", String(pending));
  };
  root.addEventListener("click", (event) => {
    const add = event.target.closest(".add-comment");
    if (add) {
      const entry = rowEntries.get(add.closest(legacy ? ".row" : ".diff-row"));
      const point = event.shiftKey && rangeAnchor ? entry.points.find((candidate) => groupKey(candidate) === groupKey(rangeAnchor.point)) ?? entry.point : entry.point;
      if (event.shiftKey && rangeAnchor && groupKey(rangeAnchor.point) === groupKey(point)) {
        const form = rangeAnchor.form; if (form.dataset.pending === "true") return;
        const range = { ...point, line: Math.min(rangeAnchor.point.line, point.line), endLine: Math.max(rangeAnchor.point.line, point.line) };
        const destination = destinationFor(range, entry.variant) ?? entry, previous = formEntry.get(form);
        form.dataset.location = JSON.stringify(range); form.querySelector(".comment-location").textContent = label(range); threadFor(destination).append(form); formEntry.set(form, destination); trimThread(previous);
        for (const candidate of entries) candidate.row.classList.toggle("range-selected", candidate.points.some((location) => groupKey(location) === groupKey(range) && location.line >= range.line && location.line <= range.endLine));
        rangeAnchor = null; form.querySelector("textarea").focus();
      } else { const form = openForm(entry, point); rangeAnchor = { point, form }; }
      return;
    }
    const cancel = event.target.closest(".cancel-comment");
    if (cancel) { const form = cancel.closest("form"); if (form.dataset.pending !== "true") closeForm(form, form.contains(document.activeElement)); return; }
    const view = event.target.closest(".comment-view"); if (!view) return;
    const id = view.dataset.id; if (pendingIds.has(id)) return;
    if (event.target.closest(".edit-comment")) {
      const comment = comments.find((candidate) => candidate.id === id); if (!comment) return;
      const entry = entries.find((candidate) => candidate.views.get(id) === view); openForm(entry, pointOf(comment), comment); return;
    }
    if (event.target.closest(".delete-comment")) {
      if (editors.has(id)) { editors.get(id).querySelector("textarea").focus(); return; }
      const previous = deleting.get(id); if (previous) previous.querySelector(".comment-tools").innerHTML = toolsHtml;
      deleting.set(id, view); view.querySelector(".comment-error").textContent = "";
      view.querySelector(".comment-tools").innerHTML = '<span class="inline-delete-label">Delete this comment?</span><button type="button" class="cancel-inline-delete cancel-delete">Cancel</button><button type="button" class="confirm-inline-delete confirm-delete" aria-busy="false">Delete</button>';
      view.querySelector(".confirm-delete").focus(); return;
    }
    if (event.target.closest(".cancel-delete")) {
      const restore = view.contains(document.activeElement); deleting.delete(id); view.querySelector(".comment-tools").innerHTML = toolsHtml; view.querySelector(".comment-error").textContent = "";
      activity(view, "clear", "delete");
      if (restore) view.querySelector(".delete-comment").focus(); return;
    }
    if (event.target.closest(".confirm-delete")) removeComment(view);
  });
  root.addEventListener("submit", async (event) => {
    const form = event.target; if (!form.matches(".comment-form")) return; event.preventDefault();
    const id = form.dataset.id; if (form.dataset.pending === "true" || !form.isConnected || (id && pendingIds.has(id))) return;
    const hadFocus = form.contains(document.activeElement), point = editorPoint(form), body = form.querySelector("textarea").value;
    const error = form.querySelector(".comment-error"); error.textContent = ""; busyForm(form, true); pendingWrites++; if (id) pendingIds.add(id); activity(form, "pending"); syncRetry(); draw();
    try {
      if (loading) await ready;
      const result = await api(id ? "/" + encodeURIComponent(id) : "", { method: id ? "PUT" : "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(id ? { body } : legacy ? { ...point, body } : { location: point, body }) });
      comments = id ? comments.map((comment) => comment.id === id ? result : comment) : [...comments, result];
      if (id) pendingIds.delete(id);
      closeForm(form, form.contains(document.activeElement) || (hadFocus && document.activeElement === document.body), "success");
    } catch (reason) { error.textContent = reason.message; activity(form, "failed"); }
    finally {
      if (id) pendingIds.delete(id); pendingWrites--; busyForm(form, false); syncRetry(); draw();
      if (form.isConnected && hadFocus && document.activeElement === document.body) form.querySelector(".submit-comment").focus();
    }
  });
  async function removeComment(view) {
    const id = view.dataset.id; if (pendingIds.has(id)) return;
    const button = view.querySelector(".confirm-delete"), cancel = view.querySelector(".cancel-delete"), hadFocus = view.contains(document.activeElement);
    pendingIds.add(id); pendingWrites++; button.disabled = true; cancel.disabled = true; button.textContent = "Deleting…"; button.setAttribute("aria-busy", "true"); view.querySelector(".comment-error").textContent = ""; activity(view, "pending", "delete"); syncRetry(); draw();
    try {
      if (loading) await ready; await api("/" + encodeURIComponent(id), { method: "DELETE" });
      const restore = view.contains(document.activeElement) || (hadFocus && document.activeElement === document.body);
      const entry = entries.find((candidate) => candidate.views.get(id) === view);
      comments = comments.filter((comment) => comment.id !== id); deleting.delete(id); activity(view, "success", "delete"); draw(); if (restore) focusEntry(entry);
    } catch (reason) { view.querySelector(".comment-error").textContent = reason.message; activity(view, "failed", "delete"); }
    finally {
      pendingIds.delete(id); pendingWrites--; syncRetry(); button.disabled = false; cancel.disabled = false; button.textContent = "Delete"; button.setAttribute("aria-busy", "false"); draw();
      if (view.isConnected && hadFocus && document.activeElement === document.body) button.focus();
    }
  }
  root.addEventListener("keydown", (event) => { if (event.key === "Enter" && (event.ctrlKey || event.metaKey) && event.target.matches(".comment-form textarea")) { event.preventDefault(); event.target.form.requestSubmit(); } });
  async function loadComments() {
    loading = true; loadStatus.hidden = false; loadMessage.textContent = "Loading comments…"; syncRetry();
    try { comments = await api(""); draw(); loadStatus.hidden = true; }
    catch (reason) { loadMessage.textContent = `Could not load comments: ${reason.message}`; }
    finally { loading = false; syncRetry(); }
  }
  draw(); let ready = legacy ? loadComments() : Promise.resolve();
  retry.onclick = () => { if (!loading && !pendingWrites) ready = loadComments(); };
}
