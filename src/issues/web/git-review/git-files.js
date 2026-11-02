import { createGitActivity } from "./git-activity.js";

export function renderGitFileToggle() {
  return '<button type="button" class="chevron git-file-toggle" aria-expanded="true" aria-label="Collapse file"><span aria-hidden="true">⌄</span></button>';
}

export function mountGitFiles(createActivity = createGitActivity) {
  const root = document.querySelector("#files") ?? document.querySelector("main"), entries = new WeakMap(), files = [], destinations = new Map();
  const filter = document.querySelector(".filter"), contents = document.querySelector(".toc"), links = [...document.querySelectorAll(".toc li")];
  let filterStatus, filterMessage, current, currentHash = "";
  const placeActivity = entry => {
    const notice = entry.activity.element; if (!notice) return;
    const target = entry.file.hidden ? entry.file : entry.content;
    notice.classList.toggle("git-filtered-activity", entry.file.hidden);
    if (notice.nextSibling !== target) target.before(notice);
  };
  const applyFilter = value => {
    filter.value = value; const term = value.trim().toLowerCase(); let matching = 0;
    for (const entry of files) {
      const visible = entry.search.includes(term), wasHidden = entry.file.hidden;
      if (visible) matching++;
      if ((!visible && entry.file.contains(document.activeElement)) || (wasHidden === visible && entry.activity.element?.contains(document.activeElement))) filter.focus({ preventScroll: true });
      if (wasHidden === visible) entry.file.hidden = !visible;
      if (entry.link && entry.link.hidden === visible) entry.link.hidden = !visible;
      if (visible && wasHidden && !entry.content.hidden) entry.activity.acknowledge(); else entry.activity.refresh();
      placeActivity(entry);
    }
    filterStatus.hidden = !term;
    const message = matching ? `${matching} of ${files.length} files` : `No files match “${value.trim()}”.`;
    if (filterMessage.textContent !== message) filterMessage.textContent = message;
  };
  const setExpanded = (entry, expanded) => {
    const restore = !expanded && entry.content.contains(document.activeElement);
    entry.content.hidden = !expanded; entry.file.classList.toggle("collapsed", !expanded);
    entry.button.setAttribute("aria-expanded", String(expanded));
    const label = `${expanded ? "Collapse" : "Expand"} changes in ${entry.name}`;
    entry.button.setAttribute("aria-label", label); entry.button.title = label;
    if (expanded) entry.activity.acknowledge();
    if (restore) entry.button.focus({ preventScroll: true });
  };
  const navigate = (entry, updateHistory = false) => {
    if (!entry) return;
    if (entry.file.hidden && filter) applyFilter("");
    setExpanded(entry, true);
    if (current !== entry) { current?.anchor?.removeAttribute("aria-current"); entry.anchor?.setAttribute("aria-current", "location"); current = entry; }
    if (updateHistory && entry.file.id) {
      const hash = "#" + entry.file.id;
      if (location.hash !== hash) history.pushState(history.state, "", hash);
      currentHash = hash;
    }
    entry.button.focus();
  };
  const followHash = () => {
    if (currentHash === location.hash) return;
    currentHash = location.hash;
    const entry = destinations.get(currentHash);
    if (entry) navigate(entry); else { current?.anchor?.removeAttribute("aria-current"); current = null; }
  };
  root.querySelectorAll(".file").forEach((file, index) => {
    const button = file.querySelector(".git-file-toggle"), content = file.querySelector(":scope > .file-content, :scope > .diff");
    if (!button || !content) return;
    const entry = { file, button, content, link: links[index], name: file.querySelector("header .path, header strong")?.textContent ?? `file ${index + 1}` };
    entry.anchor = entry.link?.querySelector("a");
    if (entry.anchor) {
      entries.set(entry.anchor, entry); entry.anchor.title = entry.name; entry.anchor.setAttribute("aria-label", entry.name);
      const boundary = entry.name.lastIndexOf("/") + 1, basename = document.createElement("span");
      basename.className = "git-file-name"; basename.textContent = entry.name.slice(boundary); basename.setAttribute("aria-hidden", "true"); entry.anchor.textContent = ""; entry.anchor.append(basename);
      if (boundary) { const directory = document.createElement("span"); directory.className = "git-file-directory"; directory.textContent = entry.name.slice(0, boundary); directory.setAttribute("aria-hidden", "true"); entry.anchor.append(directory); }
    }
    if (file.id) destinations.set("#" + file.id, entry);
    entry.search = entry.name.toLowerCase(); files.push(entry);
    content.id = `git-file-content-${index}`; button.setAttribute("aria-controls", content.id);
    entry.activity = createActivity({ name: entry.name, className: "git-file-activity", controls: content.id, prefix: () => file.hidden ? entry.name + ": " : "", insert: notice => { entries.set(notice, entry); content.before(notice); }, collapsed: () => content.hidden || file.hidden, reveal: () => { if (file.hidden && filter) applyFilter(""); setExpanded(entry, true); }, focus: () => file.hidden && filter ? filter : button });
    entries.set(file, entry); setExpanded(entry, !file.classList.contains("collapsed"));
  });
  if (filter) {
    if (!root.id) root.id = "git-files";
    filter.type = "search"; filter.setAttribute("aria-label", "Filter changed files"); filter.setAttribute("aria-controls", root.id);
    const hint = document.createElement("span"); hint.className = "git-filter-hint"; hint.id = "git-file-filter-hint"; hint.textContent = "Enter opens the first matching file"; filter.after(hint);
    filter.setAttribute("aria-describedby", [filter.getAttribute("aria-describedby"), hint.id].filter(Boolean).join(" "));
    filterStatus = document.createElement("div"); filterStatus.className = "git-filter-status"; filterStatus.hidden = true;
    filterStatus.innerHTML = '<span role="status" aria-live="polite" aria-atomic="true"></span><button type="button" aria-label="Clear file filter">Clear filter</button>';
    filterMessage = filterStatus.querySelector("span"); root.prepend(filterStatus);
    filter.addEventListener("input", () => applyFilter(filter.value));
    filter.addEventListener("keydown", event => {
      if (event.key !== "Enter" || event.isComposing || event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
      const entry = files.find(candidate => !candidate.file.hidden); if (!entry) return;
      event.preventDefault(); navigate(entry, true);
    });
    if (filter.value) applyFilter(filter.value);
  }
  contents?.addEventListener("click", event => {
    if (event.defaultPrevented || event.button !== 0 || event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
    const entry = entries.get(event.target.closest("a")); if (!entry) return;
    event.preventDefault(); navigate(entry, true);
  });
  if (destinations.size) {
    let queued = false;
    const scheduleHash = () => { if (queued) return; queued = true; requestAnimationFrame(() => { queued = false; followHash(); }); };
    window.addEventListener("hashchange", scheduleHash); window.addEventListener("popstate", scheduleHash);
    const initialHash = () => { if (document.activeElement === document.body) followHash(); };
    if (document.readyState === "complete") initialHash(); else window.addEventListener("load", () => requestAnimationFrame(initialHash), { once: true });
  }
  root.addEventListener("click", event => {
    if (event.target.closest(".git-filter-status button")) { applyFilter(""); filter.focus(); return; }
    const comments = event.target.closest(".git-file-comments");
    if (comments) {
      const entry = entries.get(comments.closest(".file")); if (!entry) return;
      setExpanded(entry, true); entry.file.dispatchEvent(new CustomEvent("git-show-file-comments", { bubbles: true })); return;
    }
    const show = event.target.closest(".git-file-activity button");
    if (show) {
      const entry = entries.get(show.closest(".git-file-activity")); if (!entry) return;
      entry.activity.show(); return;
    }
    const button = event.target.closest(".git-file-toggle"), entry = button && entries.get(button.closest(".file"));
    if (entry) setExpanded(entry, entry.content.hidden);
  });
  root.addEventListener("change", event => {
    const checkbox = event.target.closest(".viewed input"), entry = checkbox && entries.get(checkbox.closest(".file"));
    if (entry) setExpanded(entry, !checkbox.checked);
  });
  root.addEventListener("git-comment-activity", event => {
    const entry = entries.get(event.target); if (!entry) return;
    const { owner, state, action } = event.detail;
    entry.activity.update(owner, state, action); placeActivity(entry);
  });
  root.addEventListener("git-file-comment-count", event => {
    const entry = entries.get(event.target); if (!entry) return;
    const { count } = event.detail;
    if (!entry.comments && count) {
      entry.comments = document.createElement("button"); entry.comments.type = "button"; entry.comments.className = "git-file-comments";
      entry.comments.setAttribute("aria-controls", entry.content.id); entry.file.querySelector("header").after(entry.comments);
    }
    if (!entry.comments) return;
    if (!count && document.activeElement === entry.comments) entry.button.focus({ preventScroll: true });
    entry.comments.hidden = !count;
    const label = `${count} comment${count === 1 ? "" : "s"}`; entry.comments.textContent = label; entry.comments.setAttribute("aria-label", `Show ${label} in ${entry.name}`);
  });
}
