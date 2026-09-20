"use strict";
const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
const {icon, icons, relative, date} = HeyBossUI;
const esc = (value) =>
  String(value ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
// Private HTTPS mobile access keeps drafts and retry IDs in this page's memory.
// No issue content is persisted on the phone; desktop HTTP retains saved drafts.
const volatileStorage = new Map();
const persistDrafts = location.protocol === "http:";
const storage = {
  get(key) {
    if (!persistDrafts) return volatileStorage.get(key) ?? null;
    try {
      return JSON.parse(localStorage.getItem(key));
    } catch {
      return null;
    }
  },
  set(key, value) {
    if (!persistDrafts) {
      volatileStorage.set(key, structuredClone(value));
      return;
    }
    try {
      localStorage.setItem(key, JSON.stringify(value));
    } catch {}
  },
  remove(key) {
    if (!persistDrafts) {
      volatileStorage.delete(key);
      return;
    }
    try {
      localStorage.removeItem(key);
    } catch {}
  },
};
let model = {
  csrf: "",
  actor: null,
  boss: { id: "human:boss", name: "Boss" },
  assignees: [],
  projects: [],
  labels: [],
  project: null,
  route: {},
  issues: [],
  detail: null,
  editor: null,
  sequence: 0,
  polling: false,
  orderVersion: 0,
  orderDragging: false,
  orderSaving: false,
  signature: "",
  creation: null,
};
let pageRequests = new AbortController();
window.addEventListener("pagehide", () => pageRequests.abort());
window.addEventListener("pageshow", (event) => {
  if (!event.persisted) return;
  pageRequests = new AbortController();
  if (model.csrf && model.project) {
    refresh(false);
    refreshInboxBadge();
  }
});
const detailCache = new Map();
let toastTimer,
  searchTimer,
  previewSequence = 0,
  pendingMutation = new Map(
    Object.entries(storage.get("hey-boss-issues-pending") || {}),
  ),
  confirmResolve = null;
icons();
const own = (id) => id && id === model.actor?.id;
function actorName(id) {
  if (!id) return "Unassigned";
  if (id === "human:boss") return model.boss.name;
  if (own(id)) return "You";
  if (id.startsWith("codex:")) return `Codex · ${id.slice(6, 14)}`;
  if (id.startsWith("claude:")) return `Claude · ${id.slice(7, 15)}`;
  return id.replace(/^human:/, "").split("@")[0];
}
const nameSegmenter =
  typeof Intl.Segmenter === "function"
    ? new Intl.Segmenter(undefined, { granularity: "grapheme" })
    : null;
function initials(name) {
  const characters = nameSegmenter
    ? Array.from(nameSegmenter.segment(name), (part) => part.segment)
    : Array.from(name);
  return characters.slice(0, 2).join("").toUpperCase();
}
function avatar(id) {
  return `<span class="avatar" title="${esc(id)}" aria-label="${esc(actorName(id))}">${esc(id === "human:boss" ? initials(actorName(id)) : own(id) ? "Y" : id.startsWith("codex:") ? "CX" : id.startsWith("claude:") ? "CL" : initials(actorName(id)))}</span>`;
}
function labelTone(name) {
  const known = {
    bug: 1,
    enhancement: 0,
    feature: 0,
    documentation: 5,
    docs: 5,
    "needs-review": 3,
    blocked: 4,
    performance: 2,
  };
  return (
    known[name.toLowerCase()] ??
    [...name].reduce((a, c) => a + c.charCodeAt(0), 0) % 6
  );
}
function label(name) {
  return `<span class="label tone-${labelTone(name)}" title="${esc(name)}">${esc(name)}</span>`;
}
function toast(message, error = false) {
  clearTimeout(toastTimer);
  const el = $("#toast");
  el.classList.toggle("error", error);
  el.innerHTML = `${icon(error ? "issue" : "check")}<span>${esc(message)}</span><button class="toast-close" aria-label="Dismiss notification">${icon("x")}</button>`;
  el.hidden = false;
  $(".toast-close", el).onclick = () => (el.hidden = true);
  toastTimer = setTimeout(() => (el.hidden = true), error ? 10000 : 4000);
}
function connection(ok) {
  $("#connection").classList.toggle("offline", !ok);
  $("#connection span").textContent = ok ? "Connected" : "Reconnecting";
}
async function post(path, data, reconnect = true) {
  const sentToken = model.csrf;
  let response;
  try {
    response = await fetch(path, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Hey-Boss-CSRF": sentToken,
      },
      body: JSON.stringify(data),
      signal: AbortSignal.any([AbortSignal.timeout(35000), pageRequests.signal]),
    });
  } catch {
    connection(false);
    throw new Error(
      "Connection lost. Your draft is saved. Try again when the server is available.",
    );
  }
  let value;
  try {
    value = await response.json();
  } catch {
    throw new Error(
      "The server returned an unexpected response. Try refreshing.",
    );
  }
  if (response.status === 403 && reconnect) {
    try {
      const bootstrap = await fetch("/api/bootstrap", {
        signal: AbortSignal.any([AbortSignal.timeout(10000), pageRequests.signal]),
      });
      const fresh = await bootstrap.json();
      if (
        bootstrap.ok &&
        fresh.ok &&
        fresh.csrf !== sentToken &&
        fresh.actor.id === model.actor.id
      ) {
        model.csrf = fresh.csrf;
        return post(path, data, false);
      }
    } catch {
      /* Preserve the original, actionable error below. */
    }
  }
  if (!response.ok || !value.ok) {
    const error = new Error(value.error?.message || "The request failed.");
    error.code = value.error?.code;
    throw error;
  }
  if (
    value.boss &&
    (data.host || model.defaultHost || "") ===
      (model.route.host || model.defaultHost || "") &&
    (value.scope === "global" || value.project?.id === model.project?.id)
  ) {
    const host = data.host || model.defaultHost || "",
      version = value.boss.version ??
        (value.scope === "global" ? value.version : null);
    if (
      model.bossHost !== host ||
      model.boss.version == null ||
      (version != null && version >= model.boss.version)
    ) {
      if (model.boss.name !== value.boss.name) {
        detailCache.clear();
        model.signature = "";
      }
      model.boss = { ...value.boss, version };
      model.bossHost = host;
      updateProfile();
    }
  }
  connection(true);
  return value;
}
const api = (
  operation,
  project = model.project?.id,
  request_id = null,
  host = model.route.host || null,
) => post("/api/action", { project, operation, request_id, host: host || null });
async function mutationKey(project, operation, host) {
  const bytes = new TextEncoder().encode(
    JSON.stringify([model.actor.id, project, operation, ...(host && host !== model.defaultHost ? [host] : [])]),
  );
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}
function persistPending() {
  storage.set(
    "hey-boss-issues-pending",
    Object.fromEntries([...pendingMutation].slice(-100)),
  );
}
async function mutate(operation, project = model.project.id, host = model.route.host || null) {
  const key = await mutationKey(project, operation, host);
  let id = pendingMutation.get(key);
  if (!id) {
    id = crypto.randomUUID();
    pendingMutation.set(key, id);
    persistPending();
  }
  try {
    const result = await api(operation, project, id, host);
    pendingMutation.delete(key);
    persistPending();
    detailCache.delete(detailKey(project,operation.number,host));
    return result;
  } catch (error) {
    if (["conflict", "invalid_input", "not_found"].includes(error.code))
      pendingMutation.delete(key);
    persistPending();
    throw error;
  }
}
function routeHash(route) {
  const p = new URLSearchParams();
  for (const [key, value] of Object.entries(route)) {
    if (
      value !== null &&
      value !== undefined &&
      value !== "" &&
      value !== false &&
      key !== "offset"
    )
      p.set(key, String(value));
  }
  return "#" + p.toString();
}
function parseRoute() {
  const resource = HeyBossRoutes.resolve();
  const params = new URLSearchParams(location.hash.slice(1));
  const project = HeyBossUI.projectId(model.project.id);
  return {
    project,
    host: params.get("host") || model.defaultHost || "",
    view: ["notice", "inbox"].includes(resource?.entity) ? "inbox" : "issues",
    notice: resource?.entity === "notice" ? resource.id : "",
    inbox_state: params.get("inbox_state") === "archive" ? "archive" : "unread",
    inbox_project: params.get("inbox_project") || "",
    inbox_search: params.get("inbox_search") || "",
    issue: resource?.entity === "issue" ? Number(resource.id) : null,
    state: ["open", "blocked", "closed", "deleted"].includes(params.get("state"))
      ? params.get("state")
      : "open",
    search: params.get("search") || "",
    owner: params.get("owner") || "all",
    label: params.get("label") || "",
  };
}
function navigate(changes, replace = false) {
  const route = { ...model.route, ...changes };
  const hash = routeHash(route);
  saveComment();
  if (replace) {
    history.replaceState(null, "", hash);
  } else if (location.hash !== hash) {
    history.pushState(null, "", hash);
  }
  renderRoute();
}
function currentProject() {
  return (
    model.projects.find((p) => p.id === model.route.project) || {
      id: model.route.project,
      name: model.route.project
        .split("/")
        .pop()
        .replace(/^named:/, ""),
      open: 0,
      blocked: 0,
      closed: 0,
      deleted: 0,
      unassigned: 0,
    }
  );
}
function updateHeader() {
  const p = model.project;
  projectPicker.update(model.projects, p);
  $("#hidden-project-banner").hidden = !p.hidden_at;
  $("#project-caption").textContent =
    p.id.startsWith("local:") || p.id.startsWith("named:") ? p.name : p.id;
  $("#copy-create-command").disabled = false;
  $("#heading-count").textContent = p.open;
  $("#open-count").textContent = p.open;
  $("#blocked-count").textContent = p.blocked || 0;
  $("#closed-count").textContent = p.closed;
  $("#deleted-count").textContent = p.deleted;
  $(".page-description").textContent =
    `${p.unassigned} unassigned ${p.unassigned === 1 ? "issue" : "issues"}`;
  document.title =
    model.detail && model.route.issue
      ? `${model.detail.issue.title} · Hey Boss`
      : `${p.name} · Issues · Hey Boss`;
  $$("[data-state]").forEach((b) => {
    const selected = b.dataset.state === model.route.state;
    b.classList.toggle("selected", selected);
    b.setAttribute("aria-selected", selected);
    b.tabIndex = selected ? 0 : -1;
  });
  renderOwnerFilter();
  updateProfile();
  renderLabelFilter();
}
function renderOwnerFilter() {
  const selected = model.route.owner;
  const ids = [
    ...new Set([
      "human:boss",
      ...model.assignees,
      ...(!["all", "mine", "unassigned"].includes(selected) ? [selected] : []),
    ]),
  ];
  $("#owner-filter").innerHTML =
    '<option value="all">Assignee</option>' +
    `<option value="mine">Assigned to me (${esc(model.boss.name)})</option><option value="unassigned">Unassigned</option>` +
    ids
      .map((id) => `<option value="${esc(id)}">${esc(actorName(id))}</option>`)
      .join("");
  $("#owner-filter").value = selected;
}
function listLabel(name) {
  return `<a class="list-label-filter" href="${esc(routeHash({ ...model.route, issue: null, label: name }))}" aria-label="Filter by label ${esc(name)}">${label(name)}</a>`;
}
function listAssignee(id, number) {
  const filter = `<a class="list-assignee-filter" href="${esc(routeHash({ ...model.route, issue: null, owner: id }))}" aria-label="Filter by assignee ${esc(actorName(id))}" title="Filter by ${esc(actorName(id))}">${avatar(id)}<span>${esc(actorName(id))}</span></a>`;
  if (id.startsWith('human:')) return filter;
  const trace = '/agents/session#' + new URLSearchParams({project:model.project.id, issue:number, agent:id});
  return `<span class="list-assignee">${filter}<a class="list-agent-trace" href="${esc(trace)}" aria-label="Open agent conversation for issue #${number}" title="Open ${esc(actorName(id))}'s conversation">${icon('arrow-right')}</a></span>`;
}
function renderLabelFilter() {
  const selected = model.route.label;
  const all = [
    ...new Set([...model.labels, ...(selected ? [selected] : [])]),
  ].sort();
  $("#label-filter").innerHTML =
    '<option value="">Labels</option>' +
    all.map((l) => `<option value="${esc(l)}">${esc(l)}</option>`).join("");
  $("#label-filter").value = selected;
}
const projectPicker = new HeyBossUI.ProjectPicker({
  onSelect(project) { navigate({project, issue:null, search:"", label:"", owner:"all", state:"open"}); },
  onVisibility(project) { setProjectVisibility(project); },
});
function projectOptions() { projectPicker.update(model.projects, model.project); }
function closeProjectMenu() { return projectPicker.close(); }
async function setProjectVisibility(project) {
  const p = model.projects.find((p) => p.id === project);
  if (!p) return;
  const restoring = Boolean(p.hidden_at);
  const button = [...$$("[data-project-visibility]")].find(
    (b) => b.dataset.projectVisibility === project,
  );
  if (button) button.disabled = true;
  try {
    await mutate(
      { action: restoring ? "restore_project" : "hide_project" },
      project,
    );
    await refreshProjects();
    if (!restoring && model.project.id === project) {
      const next = model.projects.find((p) => !p.hidden_at);
      if (next)
        navigate({
          project: next.id,
          issue: null,
          search: "",
          label: "",
          owner: "all",
          state: "open",
        });
    }
    projectOptions();
    toast(
      restoring
        ? `${p.name} restored`
        : `${p.name} hidden. Restore it from Hidden projects.`,
    );
  } catch (error) {
    toast(error.message, true);
    if (button) button.disabled = false;
  }
}
$("#restore-current-project").onclick = () =>
  setProjectVisibility(model.project.id);
function listOperation() {
  return {
    action: "list",
    state: model.route.state,
    mine: model.route.owner === "mine",
    unassigned: model.route.owner === "unassigned",
    assignee: ["all", "mine", "unassigned"].includes(model.route.owner)
      ? null
      : model.route.owner,
    labels: model.route.label ? [model.route.label] : [],
    search: model.route.search || null,
    limit: 50,
    offset: 0,
    all: true,
  };
}
function emptyState() {
  const filtered =
    model.route.search || model.route.label || model.route.owner !== "all";
  const state = model.route.state;
  const title = filtered
    ? "No matching issues"
    : state === "blocked" ? "No blocked issues" : state === "closed"
      ? "Nothing closed yet"
      : state === "deleted"
        ? "No deleted issues"
        : "A clear place to start";
  const description = filtered
    ? "Try another search or clear your filters."
    : state === "blocked" ? "Issues that need help to proceed will appear here. Reopen them when the blocker is resolved." : state === "closed"
      ? "Completed work will appear here."
      : state === "deleted"
        ? "Deleted issues can be restored from this view."
        : "Create an issue, add some context, and let the work begin.";
  return `<div class="empty-state"><div class="empty-icon">${icon(filtered ? "search" : state === "blocked" ? "blocked" : state === "closed" ? "closed" : state === "deleted" ? "trash" : "issue")}</div><h2>${title}</h2><p>${description}</p>${filtered ? '<button class="button" data-empty="clear">Clear filters</button>' : state === "open" ? `<button class="button primary" data-empty="create">${icon("plus")}Create your first issue</button>` : ""}</div>`;
}
function listPullRequests(issue) {
  return (issue.pull_requests || [])
    .map((pr) => {
      let title = pr.url;
      try {
        const url = new URL(pr.url),
          match = url.pathname.match(/^\/([^/]+)\/([^/]+)\/pull\/(\d+)\/?$/);
        title = match
          ? `${match[1]}/${match[2]}#${match[3]}`
          : `${url.host}${url.pathname}${url.search}`;
      } catch {
        /* Keep old attached links readable if URL parsing fails. */
      }
      const purpose = prPurposeLabel(pr.purpose);
      return `<a class="issue-pr-link" href="${esc(pr.url)}" target="_blank" rel="noopener noreferrer" title="${esc(pr.url)} · ${purpose}" aria-label="Open pull request ${esc(title)} · ${purpose}">${icon("link")}<span class="pr-link-title">${esc(title)}</span><span class="pr-purpose-label">${purpose}</span></a>`;
    })
    .join("");
}
function renderList(result) {
  if (model.orderDragging) return;
  const creation = model.creation;
  const createdHere =
    creation &&
    creation.project === model.project.id &&
    creation.host === model.route.host;
  if (
    createdHere &&
    creation.reveal &&
    !creation.filtersChecked &&
    !result.issues.some((issue) => issue.number === creation.number)
  ) {
    creation.filtersChecked = true;
    navigate({ state: "open", owner: "all", label: "", search: "" }, true);
    return;
  }
  const listContext = JSON.stringify([model.project.id, model.route.host]);
  const focused = document.activeElement;
  const focusedRow = focused?.closest("#issue-list .issue-row");
  const savedFocus =
    focusedRow && model.listContext === listContext
      ? {
          number: focusedRow.dataset.issueNumber,
          tag: focused.tagName,
          href: focused.getAttribute("href"),
          move: focused.dataset.moveIssue,
          top: focused.getBoundingClientRect().top,
        }
      : null;
  model.listContext = listContext;
  model.orderVersion = result.order_version;
  model.issues = result.issues;
  model.signature = JSON.stringify(result.issues);
  $("#issue-list").classList.toggle("large-list", result.issues.length > 300);
  $("#issue-list").innerHTML = result.issues.length
    ? result.issues
        .map(
          (i) =>
            `<article class="issue-row" data-issue-number="${i.number}"><button type="button" class="issue-order-handle" aria-keyshortcuts="ArrowUp ArrowDown" data-move-issue="${i.number}" aria-label="Reorder issue #${i.number}: ${esc(i.title)}" title="Drag to reorder. Use ↑ or ↓ when focused.">${icon("grip")}</button><span class="issue-state ${i.deleted_at ? "deleted" : i.state === "open" && i.draft ? "draft" : i.state}">${icon(i.deleted_at ? "trash" : i.state === "blocked" ? "blocked" : i.state === "closed" ? "closed" : i.draft ? "edit" : "issue")}</span><div class="issue-row-main"><div class="issue-title-line"><a class="issue-title" data-issue="${i.number}" href="${esc(routeHash({ ...model.route, issue: i.number }))}">${esc(i.title)}</a>${i.draft ? '<span class="draft-badge" title="Agents skip drafts until they are marked ready">Draft</span>' : ""}${i.labels.map(listLabel).join("")}</div><div class="issue-meta"><span class="issue-number">#${i.number}</span><span>${i.state === "closed" ? `closed ${i.closed_at ? `<a class="issue-time-link" data-issue="${i.number}" href="${esc(routeHash({ ...model.route, issue: i.number }))}" aria-label="Open issue #${i.number}, closed ${esc(new Date(i.closed_at).toLocaleString())}">${date(i.closed_at)}</a>` : ""}${i.closed_by ? ` by ${esc(actorName(i.closed_by))}` : ""}` : `opened ${date(i.created_at)} by ${esc(actorName(i.created_by))}`}</span>${agentLaunchCount(i)}${listPullRequests(i)}${IssueSubtasks.list(i)}</div></div><div class="issue-row-end">${i.assignee ? listAssignee(i.assignee, i.number) : ""}${i.comment_count ? `<span class="comment-count" title="${i.comment_count} comments">${icon("comment")}${i.comment_count}</span>` : ""}</div></article>`,
        )
        .join("")
    : emptyState();
  $("#list-footer").hidden = !result.issues.length;
  $("#list-summary").textContent = result.issues.length
    ? `${result.issues.length} ${result.issues.length === 1 ? "issue" : "issues"}`
    : "No issues";
  $("#issue-list").removeAttribute("aria-busy");
  if (savedFocus && !$("#list-view").hidden) {
    const row = $(
      `[data-issue-number="${CSS.escape(savedFocus.number)}"]`,
      $("#issue-list"),
    );
    const control = row &&
      $$("a, button", row).find((el) =>
        savedFocus.move
          ? el.dataset.moveIssue === savedFocus.move
          : el.tagName === savedFocus.tag &&
            el.getAttribute("href") === savedFocus.href,
      );
    const target = control || row?.querySelector(".issue-title");
    if (target) {
      target.focus({ preventScroll: true });
      window.scrollBy({
        top: target.getBoundingClientRect().top - savedFocus.top,
        behavior: "instant",
      });
    } else $("#issue-search").focus();
  }
  if (createdHere) {
    const row = $(`[data-issue-number="${creation.number}"]`, $("#issue-list"));
    if (row) {
      row.classList.add("issue-created");
      if (creation.reveal) {
        creation.reveal = false;
        row.querySelector(".issue-title").focus({ preventScroll: true });
        row.scrollIntoView({ block: "nearest" });
      }
      if (!creation.timer)
        creation.timer = setTimeout(() => {
          if (model.creation !== creation) return;
          model.creation = null;
          $$(".issue-created").forEach((el) =>
            el.classList.remove("issue-created"),
          );
        }, 4000);
    }
  }
}
$("#issue-list").onclick = (e) => {
  const empty = e.target.closest("[data-empty]");
  if (empty?.dataset.empty === "create") openEditor();
  if (empty?.dataset.empty === "clear")
    navigate({ search: "", label: "", owner: "all" });
};
let prefetchTimer;
$("#issue-list").addEventListener("pointerover", (e) => {
  const link = e.target.closest("[data-issue]");
  if (!link) return;
  clearTimeout(prefetchTimer);
  const project = model.project.id,
    number = Number(link.dataset.issue),
    host = model.route.host,
    key = detailKey(project,number,host);
  if (detailCache.has(key)) return;
  prefetchTimer = setTimeout(
    () =>
      api({ action: "view", number }, project, null, host)
        .then((value) => {
          if (detailCache.size > 40) detailCache.clear();
          detailCache.set(key, { value, at: Date.now() });
        })
        .catch(() => {}),
    100,
  );
});
$("#issue-search").oninput = () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(
    () => navigate({ search: $("#issue-search").value }, true),
    160,
  );
};
$("#label-filter").onchange = () =>
  navigate({ label: $("#label-filter").value });
$("#owner-filter").onchange = () =>
  navigate({ owner: $("#owner-filter").value });
$$("[data-state]").forEach(
  (b) => (b.onclick = () => navigate({ state: b.dataset.state })),
);
$("#refresh").onclick = () => refresh(false);
async function refreshProjects(project = model.project.id) {
  const result = await api(
    { action: "projects", include_hidden: true },
    project,
  );
  model.projects = result.projects;
  if (model.project.id !== project) return;
  model.labels = result.labels;
  model.assignees = result.assignees || [];
  model.project = currentProject();
  updateHeader();
  if (!$("#project-menu").hidden) projectOptions();
}
async function renderRoute() {
  closeIssueTagPicker();
  const sequence = ++model.sequence;
  const previous = model.project?.id;
  model.route = parseRoute();
  updateAppNavigation();
  $("#inbox-view").hidden = model.route.view !== "inbox";
  $("#project-settings-trigger").hidden = model.route.view === "inbox";
  $(".project-control").hidden = model.route.view === "inbox";
  if (model.route.view === "inbox") {
    closeProjectMenu();
    model.detail = null;
    for (const selector of [
      "#list-view",
      "#detail-view",
      "#issue-heading",
      "#hidden-project-banner",
    ])
      $(selector).hidden = true;
    await renderInboxRoute(sequence);
    return;
  }
  if (model.activeHost !== model.route.host) {
    try {
      const projects = await api(
        { action: "projects", include_hidden: true },
        model.route.project,
        null,
        model.route.host || null,
      );
      if (sequence !== model.sequence) return;
      model.projects = projects.projects;
      model.activeHost = model.route.host;
      model.labels = projects.labels;
      model.assignees = projects.assignees || [];
      detailCache.clear();
    } catch (error) {
      toast(error.message, true);
      return;
    }
  }
  clearTimeout(searchTimer);
  $("#issue-search").value = model.route.search;
  model.project = currentProject();
  model.detail = null;
  model.signature = "";
  updateHeader();
  const detail = !!model.route.issue;
  $("#list-view").hidden = detail;
  $("#detail-view").hidden = !detail;
  $("#issue-heading").hidden = detail;
  $("#issue-heading").classList.toggle("detail-page", detail);
  if (previous !== model.project.id) {
    model.labels = [];
    model.assignees = [];
    $("#issue-list").innerHTML =
      '<div class="loading-state"><span class="spinner"></span>Loading issues…</div>';
    refreshProjects(model.project.id).catch((e) => toast(e.message, true));
  }
  if (detail) {
    const key = detailKey(model.project.id,model.route.issue);
    const cached = detailCache.get(key);
    if (cached && Date.now() - cached.at < 30000) renderDetail(cached.value);
    else
      $("#detail-view").innerHTML =
        '<div class="loading-state"><span class="spinner"></span>Opening issue…</div>';
    try {
      const value = await api({ action: "view", number: model.route.issue });
      if (sequence !== model.sequence) return;
      detailCache.set(key, { value, at: Date.now() });
      if (!cached || Date.now() - cached.at >= 30000 || JSON.stringify(cached.value.issue) !== JSON.stringify(value.issue) || IssueSubtasks.signature(cached.value) !== IssueSubtasks.signature(value)) {
        const input = $("#comment-body"), focus = input === document.activeElement ? {start:input.selectionStart,end:input.selectionEnd,direction:input.selectionDirection,top:input.getBoundingClientRect().top} : null;
        saveComment();renderDetail(value);
        if (focus) {const next=$("#comment-body");next?.focus({preventScroll:true});next?.setSelectionRange(focus.start,focus.end,focus.direction);if(next)window.scrollBy(0,next.getBoundingClientRect().top-focus.top);}
      }
    } catch (e) {
      if (sequence !== model.sequence) return;
      $("#detail-view").innerHTML =
        `<button class="back-link" data-back>${icon("arrow-left")}All issues</button><div class="empty-state"><div class="empty-icon">${icon("issue")}</div><h2>Unable to open this issue</h2><p>${esc(e.message)}</p><button class="button" data-retry>Try again</button></div>`;
    }
  } else {
    try {
      const result = await api(listOperation());
      if (sequence !== model.sequence) return;
      renderList(result);
    } catch (e) {
      if (sequence !== model.sequence) return;
      $("#issue-list").innerHTML =
        `<div class="empty-state"><div class="empty-icon">${icon("issue")}</div><h2>Issues are unavailable</h2><p>${esc(e.message)}</p><button class="button" id="retry-list">Try again</button></div>`;
      $("#retry-list").onclick = () => refresh(false);
    }
  }
}
async function refresh(quiet = true) {
  if (model.route.view === "inbox") {
    await refreshInbox(quiet);
    return;
  }
  if (
    model.orderDragging ||
    model.orderSaving ||
    model.polling ||
    !model.csrf ||
    document.hidden ||
    $("#editor-dialog").open ||
    $("#confirm-dialog").open ||
    $("#transfer-dialog").open ||
    !!$(".issue-overflow[open]")
  )
    return;
  model.polling = true;
  const sequence = model.sequence,
    project = model.project.id,
    bossName = model.boss.name;
  $("#refresh").classList.add("busy");
  try {
    const [projects, result] = await Promise.all([
      api({ action: "projects", include_hidden: true }, project),
      api(
        model.route.issue
          ? { action: "view", number: model.route.issue }
          : listOperation(),
        project,
      ),
    ]);
    if (sequence !== model.sequence || model.orderDragging || model.orderSaving)
      return;
    $$("time[datetime]").forEach((time) => {
      const text = relative(Date.parse(time.dateTime));
      if (time.textContent !== text) time.textContent = text;
    });
    model.projects = projects.projects;
    model.labels = projects.labels;
    model.assignees = projects.assignees || [];
    model.project = currentProject();
    updateHeader();
    if (!$("#project-menu").hidden) projectOptions();
    if (model.route.issue) {
      model.orderVersion = result.order_version ?? model.orderVersion;
      if (model.detail && result.issue.agent_launch_count !== model.detail.issue.agent_launch_count) {
        model.detail.issue.agent_launch_count = result.issue.agent_launch_count;
        detailCache.delete(detailKey(project, result.issue.number));
        const badge = $(".sidebar .agent-launch-count");
        if (badge) {
          const count = result.issue.agent_launch_count || 0;
          const label = `${count} agent ${count === 1 ? "launch" : "launches"}`;
          $("span", badge).textContent = label;
          badge.setAttribute("aria-label", `${label}. ${$(".agent-launch-help", badge).textContent}`);
        }
      }
      if (model.detail && bossName !== model.boss.name) {
        if (quiet) showUpdate();
        else renderDetail(result);
      } else if (
        model.detail &&
        (result.issue.version !== model.detail.issue.version || JSON.stringify(result.artifacts) !== JSON.stringify(model.detail.artifacts) || IssueSubtasks.signature(result) !== IssueSubtasks.signature(model.detail))
      ) {
        if (quiet) showUpdate();
        else renderDetail(result);
      }
    } else {
      model.orderVersion = result.order_version;
      if (JSON.stringify(result.issues) !== model.signature) renderList(result);
    }
    connection(true);
    if (!quiet) toast("Up to date");
  } catch (e) {
    connection(false);
    if (!quiet) toast(e.message, true);
  } finally {
    model.polling = false;
    $("#refresh").classList.remove("busy");
  }
}
function detailKey(project, number, host = model.route.host) {
  return JSON.stringify([host || model.defaultHost || "",project,number]);
}
function draftKey(kind, project, number = "new", host = model.route.host) {
  const scope = host && host !== model.defaultHost ? `:host:${encodeURIComponent(host)}` : "";
  return `hey-boss-issues:${kind}:${project}:${number}${scope}`;
}
function saveComment() {
  if (!model.detail) return;
  const input = $("#comment-body");
  if (input)
    storage.set(
      draftKey("comment", model.project.id, model.detail.issue.number, model.activeHost ?? model.route.host),
      input.value,
    );
}
function showUpdate() {
  detailCache.delete(detailKey(model.project.id,model.route.issue));
  if ($("#update-banner")) return;
  $("#detail-view").insertAdjacentHTML(
    "afterbegin",
    `<div class="update-banner" id="update-banner"><span>This issue was updated by another session.</span><button data-reload>Load changes</button></div>`,
  );
}
function agentLaunchCount(issue, showZero = false) {
  const count = issue.agent_launch_count || 0;
  if (!count && !showZero) return "";
  const label = `${count} agent ${count === 1 ? "launch" : "launches"}`;
  const help = "Worker agent process launches, including retries and resumed sessions. Reservations and failed process starts are excluded. This is a count, not a limit.";
  return `<span class="agent-launch-count" tabindex="0" aria-label="${label}. ${help}">${icon("refresh")}<span>${label}</span><span class="agent-launch-help" aria-hidden="true">${help}</span></span>`;
}

function positionLaunchHelp(badge) {
  const help = $(".agent-launch-help", badge);
  badge.classList.remove("launch-help-below");
  const bounds = badge.getBoundingClientRect();
  help.style.left = `${Math.max(16, Math.min(bounds.left, innerWidth - help.offsetWidth - 16)) - bounds.left}px`;
  const panel = badge.closest(".issue-panel");
  if (help.getBoundingClientRect().top < Math.max(16, panel?.getBoundingClientRect().top || 0)) badge.classList.add("launch-help-below");
}
for (const type of ["pointerover", "focusin"]) document.addEventListener(type, event => {
  const badge = event.target.closest(".agent-launch-count");
  if (badge && !badge.contains(event.relatedTarget)) {
    badge.classList.remove("launch-help-dismissed");
    positionLaunchHelp(badge);
  }
});
window.addEventListener("resize", () => $$(".agent-launch-count:hover, .agent-launch-count:focus").forEach(positionLaunchHelp));
document.addEventListener("keydown", event => {
  if (event.key !== "Escape") return;
  const badges = $$(".agent-launch-count:hover, .agent-launch-count:focus");
  if (!badges.length) return;
  badges.forEach(badge => badge.classList.add("launch-help-dismissed"));
  event.preventDefault();
  event.stopImmediatePropagation();
}, true);

function assigneeActions(issue) {
  if (issue.draft) return '<p>Mark ready before assigning this issue.</p>';
  if (issue.state !== "open" || issue.deleted_at) return "";
  return `<div class="assignee-actions">${own(issue.assignee) ? "" : `<button class="button small" data-action="assign_boss">Assign to ${esc(model.boss.name)}</button>`}${issue.assignee ? '<button class="button small" data-action="unassign">Unassign</button>' : ""}</div>`;
}
function renderIssueComment(comment, deleted) {
  const header = `${avatar(comment.author)}<strong>${esc(actorName(comment.author))}</strong><span>commented ${date(comment.created_at)}</span>`;
  const action = deleted ? "" : `<button type="button" class="comment-resolve" data-resolve-comment="${comment.id}" data-resolved="${!comment.resolved}" aria-label="${comment.resolved ? "Unresolve" : "Resolve"} comment ${comment.id}">${comment.resolved ? "Unresolve" : "Resolve"}</button>`;
  const body = `<div class="comment-body markdown">${comment.body_html}</div>`;
  if (comment.resolved) {
    return `<article class="comment-card resolved-comment" data-comment-id="${comment.id}"><div class="resolved-comment-heading"><span>Resolved</span>${action}</div><details><summary class="comment-header">${header}<span class="resolved-comment-hint">Show comment</span></summary>${body}</details></article>`;
  }
  return `<article class="comment-card" data-comment-id="${comment.id}"><div class="comment-header">${header}${action}</div>${body}</article>`;
}
async function resolveComment(button) {
  const project = model.project.id, number = model.detail.issue.number, host = model.route.host;
  const commentId = Number(button.dataset.resolveComment), resolved = button.dataset.resolved === "true";
  button.disabled = true;
  saveComment();
  try {
    const result = await mutate({action: "resolve_comment", number, comment_id: commentId, resolved}, project, host);
    if (model.project.id !== project || model.route.issue !== number || model.route.host !== host) return;
    // Update just this card: preserve the draft, preview, activity and scroll.
    const comment = model.detail.comments.find((c) => c.id === commentId);
    comment.resolved = resolved;
    model.detail.issue = {...model.detail.issue, ...result.issue};
    const card = button.closest("[data-comment-id]");
    card.outerHTML = renderIssueComment(comment, false);
    const replacement = $(`[data-comment-id="${commentId}"]`);
    replacement.querySelector("[data-resolve-comment]").onclick = (event) => resolveComment(event.currentTarget);
    secureLinks();
    replacement.querySelector("summary, [data-resolve-comment]")?.focus({preventScroll: true});
    $("[data-issue-version]").textContent = `Revision ${result.issue.version}`;
  } catch (error) {
    toast(error.message, true);
    button.disabled = false;
  }
}
function draftUnavailable(issue, enabled) {
  if (issue?.state && issue.state !== "open") return "Reopen this issue before moving it to draft.";
  if (issue?.assignee) return "Unassign this issue before moving it to draft.";
  if (enabled === false) return "Drafts are disabled in project settings.";
  return "";
}
function renderReadiness(value) {
  const i = value.issue;
  if (i.state === "blocked" && !i.deleted_at) return `<div class="side-section"><h2 class="side-heading">Readiness${icon("blocked")}</h2><strong class="readiness-status">Blocked · pickup paused</strong><p>Resolve the blocker and reopen this issue to let agents continue.</p></div>`;
  if (i.deleted_at) return "";
  const reason = draftUnavailable(i, value.drafts_enabled);
  return `<div class="side-section issue-readiness"><h2 class="side-heading">Readiness${icon("edit")}</h2><strong class="readiness-status">${i.state === "closed" ? "Completed" : i.draft ? "Draft · not ready for agents" : i.assignee ? "Assigned" : "Ready for agents"}</strong><p id="readiness-help">${i.state === "closed" ? reason : i.draft ? "Keep refining the scope. Mark ready when this issue can be picked up." : reason || "Move to draft to pause agent pickup while you refine the scope."}</p>${i.draft ? "" : `<button type="button" class="button" data-draft-action="draft" aria-describedby="readiness-help" ${reason ? "disabled" : ""}>${icon("edit")}Move to draft</button>`}${i.plan ? `<div class="issue-plan"><h3>Linked plan</h3><code>${esc(i.plan.path)}</code><p>${esc(i.plan.host)} · File changes sync to this issue.</p>${i.draft ? "<p>Marking ready syncs the latest file first. The plan must be reachable.</p>" : ""}</div>` : ""}</div>`;
}
function issueStateActions(issue) {
  const reopen = issue.state !== "open";
  let buttons = `<button type="button" class="button" data-action="${reopen ? "reopen" : "close"}">${icon(reopen ? "issue" : "closed")}${reopen ? "Reopen issue" : "Close issue"}</button>`;
  if (issue.state === "open") buttons += `<button type="button" class="button" data-action="block">${icon("blocked")}Block issue</button>`;
  if (issue.state === "blocked") buttons += `<button type="button" class="button" data-action="close">${icon("closed")}Close issue</button>`;
  return buttons;
}
function renderDraftNotice(issue) {
  if (issue.state === "blocked" && !issue.deleted_at) return `<section class="blocked-notice" aria-label="Blocked issue"><div>${icon("blocked")}</div><div><h2>This issue is blocked</h2><p>Workers won’t pick up this issue. Review the activity and comments, resolve the blocker or ask for help, then reopen to continue.</p></div><button type="button" class="button" data-action="reopen">${icon("refresh")}Reopen issue</button></section>`;
  if (!issue.draft || issue.deleted_at || issue.state !== "open") return "";
  return `<section class="draft-notice" aria-label="Draft readiness"><div class="draft-notice-icon">${icon("edit")}</div><div><h2>This issue is a draft</h2><p>Agents won’t pick up this issue until you mark it ready.</p><p id="draft-error" class="form-error" role="alert" hidden></p></div><button type="button" class="button primary" data-draft-action="ready">${icon("check")}Mark ready</button></section>`;
}
function renderDetail(value) {
  if (value.moved_to) {
    saveComment();
    const destination = value.moved_to;
    const oldKey = draftKey("comment", model.project.id, value.issue.number);
    const draft = storage.get(oldKey);
    if (draft && !storage.get(draftKey("comment", destination.project.id, destination.number))) {
      storage.set(draftKey("comment", destination.project.id, destination.number), draft);
      storage.remove(oldKey);
    }
    navigate({project: destination.project.id, issue: destination.number, state: "open", owner: "all", label: "", search: ""}, true);
    toast("This issue moved to " + destination.project.name);
    return;
  }
  model.detail = value;
  model.orderVersion = value.order_version ?? model.orderVersion;
  const i = value.issue;
  const deleted = !!i.deleted_at;
  const state = deleted ? "deleted" : i.state === "open" && i.draft ? "draft" : i.state;
  const authored = esc(actorName(i.created_by));
  const description =
    i.body_html || '<p class="muted-text">No description provided.</p>';
  $("#detail-view").innerHTML =
    `<button class="back-link" data-back>${icon("arrow-left")}All issues</button>${IssueSubtasks.parent(i)}<div class="detail-top"><h1>${esc(i.title)} <span class="detail-number">#${i.number}</span></h1><div class="detail-heading-actions">${deleted ? "" : `<button type="button" class="button" data-create-subtask aria-keyshortcuts="Shift+N" title="Add subtask (Shift+N)">${icon("plus")}Add subtask</button><button class="button" data-edit>${icon("edit")}Edit</button>`}<button class="icon-button" data-copy aria-label="Copy issue link" title="Copy issue link">${icon("link")}</button></div></div><div class="detail-meta"><span class="state-pill ${state}">${icon(deleted ? "trash" : i.state === "blocked" ? "blocked" : i.state === "closed" ? "closed" : i.draft ? "edit" : "issue")}${deleted ? "Deleted" : i.state === "blocked" ? "Blocked" : i.state === "closed" ? "Closed" : i.draft ? "Draft" : "Open"}</span><span><strong>${authored}</strong> opened this issue ${date(i.created_at)}</span><span>·</span><span>${value.comments.length}${value.more_comments ? "+" : ""} comments</span></div>${renderDraftNotice(i)}<div class="detail-layout"><div class="detail-main"><article class="comment-card"><div class="comment-header">${avatar(i.created_by)}<strong>${authored}</strong><span>opened ${date(i.created_at)}</span><span class="author-badge">Author</span></div><div class="comment-body markdown">${description}</div></article><section id="issue-attachments"></section>${IssueSubtasks.card(value)}<div class="history-section"><button class="history-toggle" id="history-toggle" aria-expanded="false">${icon("clock")}View activity</button><div id="activity-timeline" hidden></div></div><div id="comments">${value.more_comments ? '<p class="field-help">Showing recent comments. View activity to read the full history.</p>' : ""}${value.comments.map((c) => renderIssueComment(c, deleted)).join("")}</div>${deleted ? `<div class="update-banner"><span>This issue is deleted. Its history is preserved.</span><button data-action="restore">Restore issue</button></div>` : `<form id="comment-form" class="comment-compose"><div class="compose-heading">${avatar(model.actor.id)}<label for="comment-body">Add a comment</label></div><div class="markdown-editor"><div class="editor-tabs" role="tablist" aria-label="Comment mode"><button type="button" id="comment-write" class="selected" role="tab" aria-selected="true">Write</button><button type="button" id="comment-preview" role="tab" aria-selected="false" tabindex="-1">Preview</button></div><textarea id="comment-body" aria-label="Your comment" rows="4" placeholder="Leave an update, ask a question, or share what you found…"></textarea><div class="markdown preview-content" id="comment-rendered" hidden></div></div><p class="form-error" id="comment-error" role="alert" hidden></p><div class="compose-actions">${issueStateActions(i)}<button class="button primary" type="submit" id="comment-submit">Comment${icon("arrow-right")}</button></div></form>`}</div><section class="sidebar" aria-label="Issue properties"><div class="side-section"><h2 class="side-heading">Assignee${icon("user")}</h2><div class="assignee-line">${i.assignee ? avatar(i.assignee) : ""}<span title="${esc(i.assignee || "")}">${esc(actorName(i.assignee))}</span></div>${assigneeActions(i)}</div>${renderReadiness(value)}${renderTagSidebar(i)}${renderPullRequests(i)}<div id="issue-artifacts" class="side-section"></div><div class="side-section" id="related-notices"><h2 class="side-heading">Related notices${icon("inbox")}</h2><p class="muted-text">Loading…</p></div><div class="side-section"><h2 class="side-heading">Project</h2><div class="side-project">${icon("folder")}${esc(model.project.name)}</div><p>${esc(model.project.id.startsWith("local:") ? "Local directory" : model.project.id.replace(/^named:/, ""))}</p></div><div class="side-section"><h2 class="side-heading">Activity</h2><p>Updated ${date(i.updated_at)}</p>${i.closed_by ? `<p>Closed by ${esc(actorName(i.closed_by))}</p>` : ""}<p data-issue-version>Revision ${i.version}</p>${agentLaunchCount(i, true)}</div><div>${deleted ? `<button class="button link-button" data-action="restore">${icon("refresh")}Restore issue</button>` : `<button class="button link-button danger" data-action="delete">${icon("trash")}Delete issue</button>`}</div></section></div>`;
  document.title = `${i.title} · Hey Boss`;
  $$('[data-resolve-comment]').forEach((button) => {
    button.onclick = (event) => resolveComment(event.currentTarget);
  });
  secureLinks();
  loadRelatedNotices(i.number, model.project.id, model.route.host);
  if (!deleted) {
    const input = $("#comment-body");
    if (!persistDrafts) {
      const note = document.createElement("p");
      note.className = "field-help";
      note.textContent = "Drafts stay in this tab. Save before reloading or closing it.";
      input.closest(".markdown-editor").after(note);
    }
    input.value =
      storage.get(draftKey("comment", model.project.id, i.number)) || "";
    input.oninput = () => {
      saveComment();
      $("#comment-submit").disabled = !input.value.trim();
    };
    $("#comment-submit").disabled = !input.value.trim();
    $("#comment-form").onsubmit = submitComment;
    $("#comment-write").onclick = () => preview("comment", false);
    $("#comment-preview").onclick = () => preview("comment", true);
  }
  HeyBossAttachments.mount(document.querySelector("#issue-attachments"), {project:model.project.id,target:{kind:"issue",id:String(i.number)},host:model.route.host,csrf:model.csrf,readonly:deleted});
  HeyBossArtifacts.mount(document.querySelector("#issue-artifacts"), {project:model.project.id,issue:i.number,host:model.route.host,csrf:model.csrf,artifacts:value.artifacts || []});
  IssueSubtasks.rendered();
  if (!deleted) {
    $(".detail-heading-actions").insertAdjacentHTML("beforeend", `<details class="issue-overflow"><summary class="icon-button" aria-label="More issue actions" title="More issue actions"><span aria-hidden="true">•••</span></summary><div class="issue-overflow-menu"><button type="button" data-transfer>${icon("folder")}Move to project…</button></div></details>`);
    $("[data-transfer]").onclick = openTransfer;
  }
  $("#history-toggle").onclick = loadHistory;
  if ($("#pr-form"))
    $("#pr-form").onsubmit = (event) => {
      event.preventDefault();
      changePullRequest("add_pull_request", $("#pr-url").value, $("#pr-purpose").value);
    };
  $$('[data-pr-purpose]').forEach((select) => {
    select.onchange = () => changePullRequest("classify_pull_request", select.dataset.prPurpose, select.value, select);
  });
}
let markdownSequence = 0;
function secureLinks() {
  // Code and tables scroll horizontally on narrow screens. Keyboard users need
  // a focus target to scroll them without moving the whole page.
  $$(".markdown pre, .markdown table").forEach((el) => {
    el.tabIndex = 0;
    el.onkeydown = (event) => {
      // WebKit doesn't consistently scroll focusable code blocks with arrows.
      if (
        event.target !== el ||
        event.altKey ||
        event.ctrlKey ||
        event.metaKey ||
        el.scrollWidth <= el.clientWidth ||
        !["ArrowLeft", "ArrowRight"].includes(event.key)
      )
        return;
      event.preventDefault();
      el.scrollLeft += event.key === "ArrowRight" ? 80 : -80;
    };
  });
  // Each body/comment/preview can define the same footnote. Keep their IDs
  // distinct, and keep anchor clicks inside the rendered Markdown's scope.
  $$(".markdown").forEach((root) => {
    const scope =
      (root.dataset.markdownScope ||= `markdown-${++markdownSequence}-`);
    root.querySelectorAll("[id]").forEach((el) => {
      if (!el.id.startsWith(scope)) el.id = scope + el.id;
    });
    root.querySelectorAll('a[href^="#"]').forEach((a) => {
      const id = a.getAttribute("href").slice(1);
      if (!id.startsWith(scope)) a.setAttribute("href", "#" + scope + id);
    });
  });
  $$('.markdown input[type="checkbox"]').forEach((input) =>
    input.setAttribute("aria-label", input.parentElement.textContent.trim()),
  );
  $$(".markdown a").forEach((a) => {
    if (a.getAttribute("href")?.startsWith("#"))
      a.onclick = (e) => {
        e.preventDefault();
        const el = document.getElementById(a.getAttribute("href").slice(1));
        el?.scrollIntoView();
      };
    else {
      a.target = "_blank";
      a.rel = "noopener noreferrer";
    }
  });
}
$("#detail-view").addEventListener("click", async (e) => {
  const button = e.target.closest("button");
  if (!button) return;
  if (button.hasAttribute("data-back")) navigate({ issue: null });
  if (button.hasAttribute("data-edit")) openEditor(model.detail.issue);
  if (button.hasAttribute("data-retry")) renderRoute();
  if (button.hasAttribute("data-reload")) {
    saveComment();
    await renderRoute();
  }
  if (button.hasAttribute("data-copy")) {
    try {
      await navigator.clipboard.writeText(location.href);
      toast("Issue link copied");
    } catch {
      toast("Copy the URL from your address bar.", true);
    }
  }
  if (button.dataset.removePr)
    changePullRequest("remove_pull_request", button.dataset.removePr);
  if (button.dataset.action) performAction(button.dataset.action, button);
});
async function performAction(action, button) {
  const i = model.detail.issue,
    project = model.project.id;
  let force = false;
  if (action === "block") {
    const yes = await confirmDialog("Block this issue?", "Blocking should be rare. Make every effort to resolve the issue first, raise questions and ask for help via hey-boss ask. Workers will pause pickup until you reopen it. Add a comment explaining the blocker.", "Block issue");
    if (!yes) return;
  }
  if (action === "delete") {
    const yes = await confirmDialog(
      "Delete this issue?",
      "The issue will move to Deleted. You can restore it later, including its comments and history.",
      "Delete issue",
    );
    if (!yes) return;
  }
  if (
    i.assignee &&
    !own(i.assignee) &&
    ["claim", "assign_boss", "unassign", "close", "block", "delete"].includes(action)
  ) {
    force = await confirmDialog(
      ["claim", "assign_boss"].includes(action)
        ? `Assign this issue to ${model.boss.name}?`
        : action === "unassign"
          ? "Unassign this issue?"
          : "Change another session’s issue?",
      `${actorName(i.assignee)} currently owns this issue. This change will clear their claim and be recorded in the activity.`,
      action === "unassign" ? "Unassign" : "Continue",
    );
    if (!force) return;
  }
  const operation = { action, number: i.number };
  if (["claim", "assign_boss", "unassign", "close", "block", "delete"].includes(action))
    operation.force = force;
  if (action === "close") operation.comment = null;
  if (action === "block") operation.comment = $("#comment-body")?.value.trim() || null;
  if (action === "reopen") operation.if_version = i.version;
  button.disabled = true;
  try {
    await mutate(operation, project);
    if (action === "block" && operation.comment && model.project.id === project && model.detail?.issue.number === i.number) {
      $("#comment-body").value = "";
      storage.remove(draftKey("comment", project, i.number));
    }
    toast(
      {
        claim: "Assigned to you",
        assign_boss: `Assigned to ${model.boss.name}`,
        unassign: "Claim released",
        block: "Issue blocked",
        close: "Issue closed",
        reopen: "Issue reopened",
        delete: "Issue moved to Deleted",
        restore: "Issue restored",
      }[action],
    );
    await Promise.all([renderRoute(), refreshProjects(project)]);
  } catch (e) {
    toast(e.message, true);
    button.disabled = false;
  }
}
async function submitComment(e) {
  e.preventDefault();
  const project = model.project.id,
    number = model.detail.issue.number,
    host = model.route.host,
    input = $("#comment-body"),
    body = input.value;
  if (!body.trim()) return;
  const button = $("#comment-submit");
  if (button.disabled) return;
  button.disabled = true;
  input.readOnly = true;
  $("#comment-error").hidden = true;
  try {
    await mutate({ action: "comment", number, body }, project,host);
    storage.remove(draftKey("comment", project, number,host));
    if (model.project.id !== project || model.route.issue !== number || model.route.host !== host) {
      toast("Comment added");
      return;
    }
    await renderRoute();
    toast("Comment added");
    $("#comment-body")?.focus();
  } catch (error) {
    input.readOnly = false;
    if (model.project.id !== project || model.route.issue !== number || model.route.host !== host) {
      toast(error.message, true);
      return;
    }
    $("#comment-error").textContent = error.message;
    $("#comment-error").hidden = false;
    button.disabled = false;
  }
}
let historyOffset = 0;
async function loadHistory() {
  const target = $("#activity-timeline");
  if (!target.hidden) {
    target.hidden = true;
    $("#history-toggle").setAttribute("aria-expanded", "false");
    return;
  }
  target.hidden = false;
  $("#history-toggle").setAttribute("aria-expanded", "true");
  target.innerHTML =
    '<div class="loading-state"><span class="spinner"></span></div>';
  historyOffset = 0;
  await historyPage(true);
}
async function historyPage(reset = false) {
  const project = model.project.id,
    number = model.detail.issue.number;
  try {
    const result = await api(
      { action: "history", number, limit: 20, offset: historyOffset },
      project,
    );
    if (model.project.id !== project || model.route.issue !== number) return;
    const target = $("#activity-timeline");
    if (reset) target.innerHTML = '<div class="timeline"></div>';
    $(".history-more", target)?.remove();
    const verbs = {
      created: "created this issue",
      edited: "edited the description or labels",
      triaged: "updated labels or ownership",
      claimed: "claimed this issue",
      unassigned: "released the claim",
      commented: "added a comment",
      comment_resolved: "resolved a comment",
      comment_unresolved: "unresolved a comment",
      blocked: "blocked this issue",
      closed: "closed this issue",
      reopened: "reopened this issue",
      deleted: "deleted this issue",
      restored: "restored this issue",
      reordered: "changed this issue’s order",
      transferred: "moved this issue from another project",
      subtask_added: "added a subtask",
      subtask_removed: "unlinked a subtask",
      parent_added: "added a parent issue",
      parent_removed: "unlinked the parent issue",
      subtask_change_conflict: "attempted a subtask change that conflicted during sync",
    };
    $(".timeline", target).insertAdjacentHTML(
      "beforeend",
      result.events
        .map(
          (event) =>
            `<div class="timeline-item"><strong>${esc(actorName(event.actor))}</strong> ${esc(verbs[event.action] || event.action)} · ${date(event.created_at)}${event.data.parent && event.data.child ? ` <span class="timeline-relationship"><a data-issue="${esc(event.data.parent)}" href="${esc(routeHash({...model.route,issue:event.data.parent}))}">#${esc(event.data.parent)}</a> → <a data-issue="${esc(event.data.child)}" href="${esc(routeHash({...model.route,issue:event.data.child}))}">#${esc(event.data.child)}</a></span>` : ""}${event.data.sync_conflict ? `<p class="muted-text">${esc(event.data.sync_conflict)}</p>` : ""}${event.data.body ? `<details><summary>Read comment</summary><pre>${esc(event.data.body)}</pre></details>` : ["edited", "triaged"].includes(event.action) ? `<details><summary>View changes</summary><pre>${esc(JSON.stringify(event.data, null, 2))}</pre></details>` : ""}</div>`,
        )
        .join(""),
    );
    if (result.next_offset !== null) {
      historyOffset = result.next_offset;
      target.insertAdjacentHTML(
        "beforeend",
        '<button class="button small history-more">Load more activity</button>',
      );
      $(".history-more", target).onclick = () => historyPage();
    }
  } catch (e) {
    toast(e.message, true);
    $("#activity-timeline").innerHTML =
      '<p class="form-error">Activity could not be loaded. Close and reopen activity to retry.</p>';
  }
}
function editorValues() {
  return {
    title: $("#editor-subject").value,
    body: $("#editor-body").value,
    labels: editorTags.values().join(", "),
    draft: $("#editor-draft").checked,
    bottom: $("#editor-bottom").checked,
    version: model.editor?.version,
    parentVersion: model.editor?.parent?.version,
  };
}
function updateEditorReadiness() {
  const ctx = model.editor;
  if (!ctx) return;
  const input = $("#editor-draft"), checked = input.checked;
  const reason = ctx.original?.draft ? "" : draftUnavailable(ctx.original, ctx.draftSettings?.drafts_enabled);
  input.disabled = ctx.busy || !!ctx.parent || !ctx.draftSettings || (!!reason && !checked);
  $("#editor-draft-control").classList.toggle("selected", checked);
  $("#editor-draft-control").classList.toggle("unavailable", input.disabled);
  $("#editor-draft-help").textContent = ctx.draftSettingsError
    ? "Could not check draft availability. Close and reopen this editor to retry."
    : !ctx.draftSettings ? "Checking draft availability…"
    : reason || (checked ? "Saved to the project. Agents skip it until you mark it ready." : "Leave unchecked to make this issue ready for agents.");
  const text = ctx.number ? checked ? "Save draft" : ctx.original.draft ? "Save & mark ready" : "Save changes"
    : ctx.parent ? "Create subtask" : checked ? "Create draft" : "Create issue";
  $("#editor-submit").innerHTML = `${text}${icon("arrow-right")}`;
  $("#editor-submit").disabled = ctx.busy || (checked && (!ctx.draftSettings || !!reason));
}
function saveEditor() {
  if (model.editor) storage.set(model.editor.key, editorValues());
}
function openEditor(issue = null, options = {}) {
  const project = model.project;
  const key = draftKey("editor", project.id, issue?.number || (options.parent ? `subtask-${options.parent.number}` : "new"));
  const draft = storage.get(key);
  model.editor = {
    project: project.id,
    number: issue?.number,
    parent: options.parent ? {...options.parent,version:draft?.parentVersion ?? options.parent.version} : null,
    host: model.route.host,
    version: draft?.version || issue?.version,
    key,
    original: issue,
    returnFocus: document.activeElement,
  };
  $("#editor-title").textContent = issue ? "Edit issue" : options.parent ? "New subtask" : "New issue";
  $("#editor-project").textContent = project.name;
  $("#editor-subject").value = draft?.title ?? issue?.title ?? "";
  $("#editor-body").value = draft?.body ?? issue?.body ?? "";
  $("#editor-draft").checked = !options.parent && (draft?.draft ?? issue?.draft ?? false);
  $("#editor-bottom").checked = draft?.bottom ?? false;
  $("#editor-bottom-control").hidden = !!issue;
  $("#editor-draft-control").hidden = !!options.parent;
  $("#editor-draft").disabled = true;
  const editor = model.editor;
  api({action:"project_settings"}, project.id, null, editor.host).then(settings => {
    if (model.editor !== editor) return;
    editor.draftSettings = settings;
    updateEditorReadiness();
  }).catch(error => {
    if (model.editor !== editor) return;
    editor.draftSettingsError = true;
    updateEditorReadiness();
    toast(error.message, true);
  });
  editorTags.set(
    (draft?.labels ?? issue?.labels.join(", ") ?? "")
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean),
  );
  $("#editor-submit").innerHTML =
    `${issue ? "Save changes" : options.parent ? "Create subtask" : "Create issue"}${icon("arrow-right")}`;
  $("#editor-submit").disabled = false;
  $("#editor-error").hidden = true;
  $("#editor-conflict").hidden = true;
  $("#conflict-replace").textContent = options.parent ? "Create using the latest parent revision" : "Save my draft over this version";
  editorBusy(false);
  preview("editor", false);
  $("#editor-dialog").showModal();
  $("#editor-subject").focus();
  if (draft) toast("Unsaved draft restored");
}
function closeEditor() {
  if (model.editor?.busy) return;
  saveEditor();
  const focus = model.editor?.returnFocus;
  $("#editor-dialog").close();
  model.editor = null;
  const target =
    focus && focus !== document.body && focus.checkVisibility()
      ? focus
      : $("#new-issue").checkVisibility()
        ? $("#new-issue")
        : $("#main");
  target.focus();
}
$("#new-issue").onclick = () => openEditor();
$("#copy-create-command").onclick = async () => {
  const quote = (value) => "'" + value.replaceAll("'", "'\\''") + "'";
  const host = model.route.host || model.defaultHost;
  const command = `hey-boss issue create --project ${quote(model.project.id)}${host ? ` --host ${quote(host)}` : ""} --title '<title>' --body '<markdown>'`;
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(command);
    } else {
      // Clipboard API is unavailable on HTTP pages outside localhost.
      const field = document.createElement("textarea");
      const focus = document.activeElement;
      field.value = command;
      field.style.cssText = "position:fixed;opacity:0;pointer-events:none";
      document.body.append(field);
      try {
        field.focus({preventScroll: true});
        field.select();
        if (!document.execCommand("copy")) throw new Error("Copy failed");
      } finally {
        field.remove();
        focus?.focus({preventScroll: true});
      }
    }
    toast("Agent command copied. Replace the title and Markdown placeholders.");
  } catch {
    toast("Could not copy the agent command. Allow clipboard access and try again.", true);
  }
};
$("#editor-close").onclick = closeEditor;
$("#editor-cancel").onclick = closeEditor;
$("#editor-dialog").addEventListener("cancel", (e) => {
  if (model.editor?.busy) {
    e.preventDefault();
    return;
  }
  saveEditor();
  model.editor = null;
});
for (const id of ["editor-subject", "editor-body", "editor-labels"])
  $("#" + id).addEventListener("input", saveEditor);
const editorTags = new TagInput(
  $("#editor-tags"),
  $("#editor-labels"),
  () => model.labels,
  saveEditor,
);
$("#editor-write").onclick = () => preview("editor", false);
$("#editor-preview").onclick = () => preview("editor", true);
async function preview(prefix, show) {
  const input = $("#" + prefix + "-body"),
    rendered = $("#" + prefix + "-rendered");
  input.hidden = show;
  rendered.hidden = !show;
  for (const suffix of ["write", "preview"]) {
    const selected = (suffix === "preview") === show;
    const b = $("#" + prefix + "-" + suffix);
    b.classList.toggle("selected", selected);
    b.setAttribute("aria-selected", selected);
    b.tabIndex = selected ? 0 : -1;
  }
  if (!show) return;
  const seq = ++previewSequence;
  rendered.innerHTML = '<span class="spinner"></span>';
  try {
    const value = await post("/api/preview", { body: input.value });
    if (seq !== previewSequence) return;
    rendered.innerHTML =
      value.html || '<p class="muted-text">Nothing to preview yet.</p>';
    secureLinks();
  } catch (e) {
    rendered.textContent = e.message;
  }
}
$("#editor-form").onsubmit = async (e) => {
  e.preventDefault();
  const ctx = model.editor;
  if (!ctx || ctx.busy || $("#editor-submit").disabled) return;
  saveEditor();
  const values = editorValues(),
    labels = [
      ...new Set(
        values.labels
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean),
      ),
    ];
  const operation = ctx.number
    ? {
        action: "edit",
        number: ctx.number,
        title: values.title,
        body: values.body,
        add_labels: labels.filter((l) => !ctx.original.labels.includes(l)),
        remove_labels: ctx.original.labels.filter((l) => !labels.includes(l)),
        if_version: ctx.version,
        ...(values.draft !== ctx.original.draft ? {draft: values.draft} : {}),
      }
    : {
        action: ctx.parent ? "create_subtask" : "create",
        ...(ctx.parent ? { number: ctx.parent.number, if_version: ctx.parent.version } : {}),
        title: values.title,
        body: values.body,
        labels,
        at_top: !values.bottom,
        ...(!ctx.parent && values.draft ? {draft:true} : {}),
      };
  editorBusy(true);
  $("#editor-error").hidden = true;
  $("#editor-conflict").hidden = true;
  try {
    const value = await mutate(operation, ctx.project, ctx.host);
    storage.remove(ctx.key);
    $("#editor-dialog").close();
    model.editor = null;
    toast(ctx.number ? values.draft ? "Draft saved" : ctx.original.draft ? "Issue marked ready" : "Issue updated" : values.draft ? "Draft created" : "Issue created");
    refreshProjects(ctx.project).catch((error) => toast(error.message, true));
    if (ctx.parent) {
      IssueSubtasks.reveal(value.issue.number);
      navigate({project:ctx.project,host:ctx.host,issue:ctx.parent.number},true);
    } else if (ctx.number)
      navigate({ project: ctx.project, issue: value.issue.number });
    else {
      clearTimeout(model.creation?.timer);
      model.creation = {
        project: ctx.project,
        host: model.route.host,
        number: value.issue.number,
        reveal: true,
        timer: null,
      };
      navigate(
        { view: "issues", project: ctx.project, issue: null, notice: "" },
        true,
      );
    }
  } catch (error) {
    $("#editor-error").textContent =
      error.code === "conflict"
        ? `${error.message}. Your draft is preserved. Compare the latest version before replacing it.`
        : error.message;
    $("#editor-error").hidden = false;
    editorBusy(false);
    if (error.code === "conflict" && (ctx.number || ctx.parent)) {
      try {
        const latest = await api(
          { action: "view", number: ctx.number || ctx.parent.number },
          ctx.project,
          null,
          ctx.host,
        );
        if (model.editor !== ctx) return;
        ctx.latest = latest.issue;
        $("#conflict-latest").textContent =
          latest.issue.title + "\n\n" + latest.issue.body;
        $("#editor-conflict").hidden = false;
      } catch (e) {
        toast(e.message, true);
      }
    }
  }
};
function editorBusy(busy) {
  if (model.editor) model.editor.busy = busy;
  $$("input,textarea,button", $("#editor-form")).forEach(
    (el) => (el.disabled = busy),
  );
  updateEditorReadiness();
}
$("#conflict-replace").onclick = () => {
  if (!model.editor?.latest) return;
  if (model.editor.parent) model.editor.parent = model.editor.latest;
  else {model.editor.version = model.editor.latest.version;model.editor.original = model.editor.latest;}
  saveEditor();
  $("#editor-form").requestSubmit();
};
let transferContext = null;
async function openTransfer() {
  const issue = model.detail.issue;
  const context = transferContext = {project: model.project.id, number: issue.number, version: issue.version, host: model.route.host};
  $(".issue-overflow").open = false;
  $("#transfer-source").textContent = `${model.project.name} · #${issue.number}`;
  $("#transfer-project").innerHTML = '<option value="">Loading projects…</option>';
  $("#transfer-project").disabled = true;
  $("#transfer-submit").disabled = true;
  $("#transfer-error").hidden = true;
  $("#transfer-dialog").showModal();
  try {
    const result = await api({action: "projects", include_hidden: false}, context.project, null, context.host);
    if (transferContext !== context || !$("#transfer-dialog").open) return;
    const projects = result.projects.filter(p => p.id !== context.project && !p.hidden_at);
    const names = new Map();
    projects.forEach(p => names.set(p.name, (names.get(p.name) || 0) + 1));
    $("#transfer-project").innerHTML = '<option value="">Choose a project…</option>' + projects.map(p => `<option value="${esc(p.id)}">${esc(p.name)}${names.get(p.name) > 1 ? ` — ${esc(p.id)}` : ""}</option>`).join("");
    $("#transfer-project").disabled = !projects.length;
    if (!projects.length) throw new Error("Create another project before moving this issue.");
    $("#transfer-project").focus();
  } catch (error) {
    if (transferContext !== context || !$("#transfer-dialog").open) return;
    $("#transfer-error").textContent = error.message;
    $("#transfer-error").hidden = false;
  }
}
$("#transfer-project").onchange = () => { $("#transfer-submit").disabled = !$("#transfer-project").value; };
$$('[data-transfer-cancel]').forEach(button => button.onclick = () => $("#transfer-dialog").close());
$("#transfer-dialog").addEventListener("close", () => { transferContext = null; $(".issue-overflow summary")?.focus(); });
$("#transfer-dialog").addEventListener("cancel", event => {
  if ($("#transfer-submit").dataset.saving) event.preventDefault();
});
$("#transfer-form").onsubmit = async event => {
  event.preventDefault();
  const context = transferContext, target = $("#transfer-project").value;
  if (!context || !target || $("#transfer-submit").dataset.saving) return;
  if (model.project.id !== context.project || model.route.issue !== context.number || model.route.host !== context.host) {
    $("#transfer-dialog").close(); return;
  }
  const submit = $("#transfer-submit");
  submit.dataset.saving = "true"; submit.disabled = true;
  submit.textContent = "Moving…";
  $("#transfer-project").disabled = true;
  $$('[data-transfer-cancel]').forEach(button => button.disabled = true);
  $("#transfer-error").hidden = true;
  saveComment();
  try {
    const result = await mutate({action: "transfer", number: context.number, destination: target, if_version: context.version}, context.project, context.host);
    const oldKey = draftKey("comment", context.project, context.number, context.host);
    const draft = storage.get(oldKey);
    if (draft) storage.set(draftKey("comment", result.project.id, result.issue.number, context.host), draft);
    storage.remove(oldKey);
    detailCache.clear(); model.signature = "";
    $("#transfer-dialog").close();
    if (model.project.id === context.project && model.route.issue === context.number && model.route.host === context.host) {
      model.detail = null;
      navigate({project: result.project.id, issue: result.issue.number, state: result.issue.state, owner: "all", label: "", search: ""});
    }
    toast(`Moved to ${result.project.name} · #${result.issue.number}`);
  } catch (error) {
    $("#transfer-error").textContent = error.message;
    $("#transfer-error").hidden = false;
  } finally {
    delete submit.dataset.saving; submit.disabled = !$("#transfer-project").value;
    submit.innerHTML = `Move issue${icon("arrow-right")}`;
    $("#transfer-project").disabled = false;
    $$('[data-transfer-cancel]').forEach(button => button.disabled = false);
  }
};
document.addEventListener("click", event => {
  if (!event.target.closest(".issue-overflow")) $$(".issue-overflow[open]").forEach(menu => menu.open = false);
});
document.addEventListener("keydown", event => {
  if (event.key === "Escape" && !$("dialog[open]")) $$(".issue-overflow[open]").forEach(menu => { menu.open = false; $("summary", menu).focus(); });
});
function confirmDialog(title, message, submit, input = false) {
  return new Promise((resolve) => {
    confirmResolve = resolve;
    $("#confirm-title").textContent = title;
    $("#confirm-message").textContent = message;
    $("#confirm-submit").textContent = submit;
    $("#confirm-input").hidden = !input;
    $("#confirm-input-label").hidden = !input;
    $("#confirm-input").value = "";
    $("#confirm-input").required = input;
    $("#confirm-error").hidden = true;
    $("#confirm-dialog").showModal();
    (input ? $("#confirm-input") : $("#confirm-cancel")).focus();
  });
}
$("#confirm-form").onsubmit = (e) => {
  e.preventDefault();
  const input = $("#confirm-input");
  if (!input.hidden && !input.value.trim()) return;
  $("#confirm-dialog").close();
  confirmResolve?.(input.hidden ? true : input.value.trim());
  confirmResolve = null;
};
$("#confirm-cancel").onclick = () => {
  $("#confirm-dialog").close();
  confirmResolve?.(false);
  confirmResolve = null;
};
$("#confirm-dialog").addEventListener("cancel", () => {
  confirmResolve?.(false);
  confirmResolve = null;
});
$("#add-project").onclick = async () => {
  closeProjectMenu();
  const name = await confirmDialog(
    "New project",
    "Choose a name for a custom project. Repositories used by agents appear automatically.",
    "Continue",
    true,
  );
  if (!name) return;
  if (name.length > 128 || /[\x00-\x1f\x7f]/.test(name)) {
    toast("Choose a project name of up to 128 characters.", true);
    return;
  }
  const id = `named:${name}`;
  if (!model.projects.some((p) => p.id === id))
    model.projects.unshift({
      id,
      name,
      open: 0,
      blocked: 0,
      closed: 0,
      deleted: 0,
      unassigned: 0,
    });
  model.route = {
    ...model.route,
    project: id,
    issue: null,
    state: "open",
    search: "",
    label: "",
    owner: "all",
  };
  model.project = currentProject();
  history.replaceState(null, "", routeHash(model.route));
  updateHeader();
  await renderRoute();
  openEditor();
};
document.addEventListener("keydown", (e) => {
  if ((e.metaKey || e.ctrlKey) && e.shiftKey && !e.altKey && e.key.toLowerCase() === "b" && $("#editor-dialog").open && !$("#confirm-dialog").open && !model.editor?.number) {
    e.preventDefault();
    if (!e.repeat && !e.isComposing && !model.editor?.busy) {
      $("#editor-bottom").checked = !$("#editor-bottom").checked;
      saveEditor();
    }
    return;
  }
  if (
    e.key === "Escape" &&
    $("#editor-dialog").open &&
    !$("#confirm-dialog").open
  ) {
    e.preventDefault();
    closeEditor();
    return;
  }
  const typing = e.target.matches(
    "input,textarea,select,[contenteditable=true]",
  );
  if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
    if (model.route.view === "inbox") {
      const form = e.target.closest("form");
      if (form) {
        e.preventDefault();
        form.requestSubmit();
      }
      return;
    }
    if ($("#editor-dialog").open) {
      e.preventDefault();
      $("#editor-form").requestSubmit();
    } else if ($("#comment-form") && $("#comment-body").value.trim()) {
      e.preventDefault();
      $("#comment-form").requestSubmit();
    }
    return;
  }
  if (e.key === "Escape" && closeProjectMenu()) {
    $("#project-trigger").focus();
    return;
  }
  if (
    e.target.matches("[role=tab]") &&
    ["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)
  ) {
    e.preventDefault();
    const tabs = $$("[role=tab]", e.target.parentElement);
    let n = tabs.indexOf(e.target);
    n =
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? tabs.length - 1
          : (n + (e.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length;
    tabs[n].focus();
    tabs[n].click();
    return;
  }
  if (typing || e.metaKey || e.ctrlKey || e.altKey || $("dialog[open]")) return;
  if (model.route.view === "inbox") {
    if (e.key === "/" && !model.route.notice) {
      e.preventDefault();
      $("#inbox-search")?.focus();
    }
    return;
  }
  if (e.shiftKey && e.key.toLowerCase() === "n" && model.route.view === "issues" && model.route.issue) {
    const add = $("[data-create-subtask]");
    if (add) {
      e.preventDefault();
      add.focus({preventScroll:true});
      add.click();
    }
    return;
  }
  if (e.key === "n" || e.key === "N") {
    e.preventDefault();
    openEditor();
  }
  if (e.key === "/" && !model.route.issue) {
    e.preventDefault();
    $("#issue-search").focus();
  }
});
window.addEventListener("beforeunload", (e) => {
  if (
    model.editor ||
    (model.route.view === "issues" && model.route.issue && $("#comment-body")?.value.trim())
  ) {
    saveEditor();
    saveComment();
    e.preventDefault();
  }
});
window.addEventListener("hashchange", () => {
  saveComment();
  renderRoute();
});
$(".skip-link").onclick = (e) => {
  e.preventDefault();
  $("#main").focus();
};
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) refresh();
});
$(".brand").onclick = (e) => {
  e.preventDefault();
  navigate({ view: "issues", issue: null, notice: "" });
};
async function boot() {
  $("#mobile-draft-help").hidden = persistDrafts;
  try {
    const response = await fetch("/api/bootstrap", {
      signal: AbortSignal.any([AbortSignal.timeout(10000), pageRequests.signal]),
    });
    const value = await response.json();
    if (!response.ok || !value.ok)
      throw new Error(value.error?.message || "Unable to connect.");
    model.defaultHost = value.backend_host || "";
    model.activeHost = model.defaultHost;
    model.csrf = value.csrf;
    model.actor = value.actor;
    model.boss = value.boss;
    model.bossHost = model.defaultHost;
    model.assignees = value.assignees || [];
    model.projects = value.projects;
    model.labels = value.labels;
    model.project = value.project;
    initGlobalSettings();
    IssueSubtasks.init();
    updateProfile();
    initInbox();
    await renderRoute();
    refreshInboxBadge();
    setInterval(() => {
      refresh();
      if (model.route.view !== "inbox") refreshInboxBadge();
    }, 5000);
  } catch (e) {
    connection(false);
    $("#issue-list").innerHTML =
      `<div class="empty-state"><div class="empty-icon">${icon("issue")}</div><h2>Let’s reconnect</h2><p>${esc(e.message)}</p><button class="button" id="reconnect">Try again</button></div>`;
    $("#reconnect").onclick = boot;
  }
}
boot();

const PR_PURPOSES = {
  unspecified: "Unspecified",
  fix: "Fix",
  prerequisite: "Prerequisite",
  "supporting-evidence": "Supporting evidence",
};
function prPurposeLabel(purpose) {
  return PR_PURPOSES[purpose] || PR_PURPOSES.unspecified;
}
function prPurposeOptions(purpose = "unspecified") {
  return Object.entries(PR_PURPOSES).map(([value, label]) => `<option value="${value}"${value === purpose ? " selected" : ""}>${label}</option>`).join("");
}
function renderPullRequests(issue) {
  return `<div class="side-section"><h2 class="side-heading">Pull requests${icon("link")}</h2><p class="field-help pr-purpose-help">Identify fixes, prerequisites, and supporting evidence. Review every linked PR.</p><div class="pr-links">${(issue.pull_requests || []).map((pr) => `<div class="pr-link"><div class="pr-link-heading"><a href="${esc(pr.url)}" target="_blank" rel="noopener noreferrer">${esc(pr.url)}</a>${issue.deleted_at ? "" : `<button type="button" class="icon-button" aria-label="Remove PR ${esc(pr.url)}" data-remove-pr="${esc(pr.url)}">${icon("x")}</button>`}</div>${issue.deleted_at ? `<span class="pr-purpose-label">${prPurposeLabel(pr.purpose)}</span>` : `<label class="pr-purpose-field"><span>Purpose</span><select class="text-input" aria-label="Purpose of PR ${esc(pr.url)}" data-pr-purpose="${esc(pr.url)}" data-saved-purpose="${esc(pr.purpose || 'unspecified')}">${prPurposeOptions(pr.purpose)}</select></label>`}</div>`).join("") || "<p>No pull requests attached.</p>"}</div>${issue.deleted_at ? "" : `<form id="pr-form"><label class="field-label" for="pr-url">Attach a PR link</label><input class="text-input" id="pr-url" type="url" required placeholder="https://github.com/…/pull/123"><label class="pr-purpose-field" for="pr-purpose"><span>Purpose</span><select class="text-input" id="pr-purpose">${prPurposeOptions()}</select></label><button class="button small" type="submit">Attach PR</button></form><p id="pr-error" class="form-error" role="alert" hidden></p>`}</div>`;
}
async function changePullRequest(action, url, purpose, control) {
  const project = model.project.id,
    number = model.detail.issue.number,
    host = model.route.host;
  const current = () => model.project?.id === project && model.route.host === host && model.route.issue === number;
  const restoreFocus = control === document.activeElement;
  const button = $("#pr-form button");
  if (button) button.disabled = true;
  if (control) control.disabled = true;
  const errorEl = $("#pr-error");
  if (errorEl) errorEl.hidden = true;
  try {
    await mutate({ action, number, url, ...(purpose ? {purpose} : {}) }, project, host);
    if (!current()) return;
    await renderRoute();
    if (!current()) return;
    if (restoreFocus) $$('[data-pr-purpose]').find(select => select.dataset.prPurpose === url)?.focus({preventScroll:true});
    toast(action === "add_pull_request" ? "PR attached" : action === "classify_pull_request" ? "PR purpose updated" : "PR removed");
  } catch (error) {
    if (!current()) return;
    if (control) control.value = control.dataset.savedPurpose;
    const el = $("#pr-error");
    if (el) {
      el.textContent = error.message;
      el.hidden = false;
    } else toast(error.message, true);
  } finally {
    if (button) button.disabled = false;
    if (control) control.disabled = false;
  }
}

window.addEventListener("hey-boss-issue-created", () => refresh(false));

$("#editor-draft").onchange = () => { updateEditorReadiness(); saveEditor(); };
$("#editor-bottom").onchange = saveEditor;
document.addEventListener("click", async event => {
  const button = event.target.closest("[data-draft-action]");
  if (!button || button.disabled) return;
  const issue = model.detail.issue, project = model.project.id, host = model.route.host;
  const ready = button.dataset.draftAction === "ready";
  const current = () => model.project?.id === project && model.route.host === host && model.route.issue === issue.number;
  saveComment();
  button.disabled = true;
  button.setAttribute("aria-busy", "true");
  const error = $("#draft-error");
  if (error) error.hidden = true;
  try {
    await mutate(ready ? {action:"undraft",number:issue.number} : {
      action:"edit",number:issue.number,draft:true,title:null,body:null,add_labels:[],remove_labels:[],if_version:issue.version
    }, project, host);
    detailCache.delete(detailKey(project, issue.number));
    if (!current()) return;
    saveComment();
    await renderRoute();
    if (!current()) return;
    const next = $("[data-draft-action]");
    (next && !next.disabled ? next : $("[data-edit]") || $("#main")).focus({preventScroll:true});
    toast(ready ? "Issue marked ready for agents" : "Issue moved to draft");
  } catch(error) {
    if (!current()) return;
    let message = $("#draft-error");
    if (!message) {
      message = document.createElement("p");
      message.id = "draft-error"; message.className = "form-error"; message.setAttribute("role", "alert");
      button.after(message);
    }
    message.textContent=error.message; message.hidden=false;
  } finally {button.disabled=false;button.removeAttribute("aria-busy");}
});
