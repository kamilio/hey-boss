"use strict";
let inboxSearchFocus = null;
let inboxTasks = [],
  inboxDetail = null,
  inboxAt = 0,
  inboxLoading = null,
  inboxSignature = "",
  inboxBusy = false,
  inboxSearchTimer,
  noticeLinkSequence = 0,
  noticeLinkTask = null,
  noticeLinkFocus = null,
  noticeLinkSelection = null,
  noticeLinkIssues = [];
const noticeDecision = (task) => ["approval", "prompt"].includes(task.kind);
const noticeReview = (task) => task.kind === "update" && !!task.commentsEnabled;
// Match Record.defaultSymbol and IconBadge in the native notification UI.
const noticeSeverity = (task) =>
  ["info", "success", "warning", "error"].includes(task.severity)
    ? task.severity
    : "neutral";
const noticeLabel = (task) =>
  ({
    info: "Information",
    success: "Success",
    warning: "Warning",
    error: "Error",
  })[noticeSeverity(task)] ||
  (task.kind === "update"
    ? "Update"
    : task.kind === "alert"
      ? "Notification"
      : "Question");
const noticeSymbols = {
  "info.circle.fill": "info",
  "checkmark.circle.fill": "success",
  "exclamationmark.triangle.fill": "warning",
  "xmark.octagon.fill": "error",
  "hammer.fill": "build",
  "chevron.left.forwardslash.chevron.right": "code",
  checklist: "test",
  "text.magnifyingglass": "review",
  "shippingbox.fill": "deploy",
  "doc.text.fill": "docs",
  "folder.fill": "folder",
  "bell.fill": "bell",
  "questionmark.bubble.fill": "question",
};
const noticeIcon = (task) => {
  const fallback =
    noticeSeverity(task) !== "neutral"
      ? noticeSeverity(task)
      : task.kind === "update"
        ? "docs"
        : task.kind === "alert"
          ? "bell"
          : "question";
  const selected = noticeSymbols[task.icon] || task.icon;
  return [
    "info",
    "success",
    "warning",
    "error",
    "build",
    "code",
    "test",
    "review",
    "deploy",
    "docs",
    "folder",
    "bell",
    "question",
  ].includes(selected)
    ? selected
    : fallback;
};
function noticeBadge(task) {
  const severity = noticeSeverity(task),
    selected = noticeIcon(task);
  // Only accept the PNG snapshots saved by snapshotIcon, never a caller file/URL.
  const custom =
    typeof task.iconData === "string" &&
    task.iconData.length <= 131072 &&
    task.iconData.startsWith("iVBORw0KGgo") &&
    /^[A-Za-z0-9+/]+={0,2}$/.test(task.iconData);
  return `<span class="notice-symbol notice-tone ${severity}" role="img" aria-label="${esc(noticeLabel(task))}" title="${esc(noticeLabel(task))}">${custom ? `<img src="data:image/png;base64,${task.iconData}" alt="" />` : icon(selected)}${severity !== "neutral" && (custom || selected !== severity) ? `<span class="notice-status-mark">${icon(severity)}</span>` : ""}</span>`;
}
const noticeStatus = (task) =>
  task.status === "pending"
    ? noticeDecision(task)
      ? "Needs an answer"
      : noticeReview(task)
        ? "Needs review"
        : "Unread"
    : task.status === "cancelled"
      ? "Cancelled"
      : noticeDecision(task)
        ? "Answered"
        : noticeReview(task)
          ? "Review finished"
          : "Read";
const noticeRoute = (id) =>
  routeHash({ ...model.route, view: "inbox", issue: null, notice: id });
const issueReferenceRoute = (issue) =>
  routeHash({
    view: "issues",
    host: issue.host || "",
    project: issue.project,
    issue: issue.number,
    state: "open",
    owner: "all",
  });
const noticeDraftKey = (task, type) => `hey-boss-inbox:${task.taskID}:${type}`;
function updateAppNavigation() {
  HeyBossUI.projectNavigation(model.route.project);
  for (const [selector, view] of [
    ["#nav-inbox", "inbox"],
    ["#nav-issues", "issues"],
  ]) {
    const selected = model.route.view === view;
    $(selector).classList.toggle("selected", selected);
    if (selected) $(selector).setAttribute("aria-current", "page");
    else $(selector).removeAttribute("aria-current");
  }
  $("#nav-inbox").href = routeHash({
    ...model.route,
    view: "inbox",
    issue: null,
    notice: "",
  });
  $("#nav-issues").href = routeHash({
    ...model.route,
    view: "issues",
    notice: "",
  });
}
function updateInboxBadge(unread) {
  $("#inbox-unread").textContent = unread;
  $("#inbox-unread").hidden = !unread;
  $("#nav-inbox").setAttribute(
    "aria-label",
    unread ? `Inbox, ${unread} unread` : "Inbox",
  );
}
async function inboxApi(action) {
  return post("/api/inbox", action);
}
async function inboxSnapshot(force = false) {
  if (!force && Date.now() - inboxAt < 4000) return inboxTasks;
  if (!inboxLoading)
    inboxLoading = inboxApi({ action: "list" })
      .then((value) => {
        inboxTasks = value.tasks;
        inboxAt = Date.now();
        updateInboxBadge(value.unread);
        return inboxTasks;
      })
      .finally(() => (inboxLoading = null));
  return inboxLoading;
}
async function refreshInboxBadge() {
  if (document.hidden) return;
  try {
    const tasks = await inboxSnapshot();
    if (model.route.view === "issues" && model.detail)
      await loadRelatedNotices(
        model.detail.issue.number,
        model.project.id,
        model.route.host,
        tasks,
      );
  } catch {
    $("#inbox-unread").hidden = true;
  }
}
function inboxEmpty(filtered) {
  return `<div class="empty-state"><div class="empty-icon">${icon(filtered ? "search" : "check")}</div><h2>${filtered ? "No matching notices" : model.route.inbox_state === "archive" ? "No activity yet" : "You’re all caught up"}</h2><p>${filtered ? "Try another search or project." : model.route.inbox_state === "archive" ? "Read updates and answers will appear here." : "Updates, questions, and reviews will appear here when they need your attention."}</p>${filtered ? '<button class="button" data-inbox-clear>Clear filters</button>' : ""}</div>`;
}
function noticeIssueChip(task) {
  return task.issue
    ? `<a class="notice-issue-chip" href="${esc(issueReferenceRoute(task.issue))}" title="${esc(task.issue.project)}">${icon("issue")}#${task.issue.number}<span>${esc(
        task.issue.project
          .split("/")
          .pop()
          .replace(/^named:/, ""),
      )}</span></a>`
    : "";
}
function renderInboxList() {
  const route = model.route,
    query = route.inbox_search.trim().toLocaleLowerCase();
  const tasks = inboxTasks.filter(
    (task) =>
      (route.inbox_state === "archive"
        ? task.status !== "pending"
        : task.status === "pending") &&
      (!route.inbox_project || task.project === route.inbox_project) &&
      (!query ||
        `${task.title} ${task.summary} ${task.project} ${task.sourceHost}`
          .toLocaleLowerCase()
          .includes(query)),
  );
  const projects = [
    ...new Set(inboxTasks.map((task) => task.project).filter(Boolean)),
  ].sort((a, b) => a.localeCompare(b));
  const unread = inboxTasks.filter((task) => task.status === "pending").length;
  inboxSignature = JSON.stringify([
    tasks,
    route.inbox_state,
    route.inbox_project,
    route.inbox_search,
  ]);
  $("#inbox-view").innerHTML =
    `<div class="page-heading inbox-heading"><div><div class="eyebrow">${icon("inbox")}Your attention, in one place</div><h1>Inbox <span class="heading-count">${unread}</span></h1><p class="page-description">${unread ? `${unread} ${unread === 1 ? "notice needs" : "notices need"} your attention.` : "A clear view of what needs you next."}</p></div><button class="button" data-inbox-refresh>${icon("refresh")}Refresh</button></div><div class="toolbar"><label class="search-field">${icon("search")}<input type="search" id="inbox-search" aria-label="Search Inbox" placeholder="Search notices…" value="${esc(route.inbox_search)}" autocomplete="off" /><kbd>/</kbd></label><label class="select-control">${icon("folder")}<select id="inbox-project-filter" aria-label="Filter Inbox by project"><option value="">All projects</option>${projects.map((project) => `<option value="${esc(project)}" ${project === route.inbox_project ? "selected" : ""}>${esc(project)}</option>`).join("")}</select></label></div><div class="glass inbox-panel"><div class="list-heading"><div class="state-tabs" role="tablist" aria-label="Inbox state"><button data-inbox-state="unread" role="tab" aria-selected="${route.inbox_state !== "archive"}" class="${route.inbox_state !== "archive" ? "selected" : ""}" tabindex="${route.inbox_state !== "archive" ? 0 : -1}">${icon("inbox")}Unread<span class="tab-count">${unread}</span></button><button data-inbox-state="archive" role="tab" aria-selected="${route.inbox_state === "archive"}" class="${route.inbox_state === "archive" ? "selected" : ""}" tabindex="${route.inbox_state === "archive" ? 0 : -1}">${icon("clock")}Activity<span class="tab-count">${inboxTasks.length - unread}</span></button></div><span class="list-sort">Newest first</span></div><div role="tabpanel" aria-label="Notices" id="notice-list">${tasks.length ? tasks.map((task) => `<article class="notice-row notice-tone ${noticeSeverity(task)} ${noticeDecision(task) || noticeReview(task) ? "needs-response" : ""}" data-notice-row="${esc(task.taskID)}">${noticeBadge(task)}<div class="notice-row-main"><div class="notice-card-heading"><span class="notice-project">${esc(task.project || "Notifications")} ·</span><a class="notice-title" href="${esc(noticeRoute(task.taskID))}">${esc(task.title || "Untitled notice")}</a></div>${task.summary ? `<p class="notice-summary">${esc(task.summary)}</p>` : ""}<div class="notice-meta"><span class="notice-severity-label">${esc(noticeLabel(task))}</span><span>·</span><span>${esc(task.sourceHost || "Source unavailable")}</span><span>·</span>${date(task.createdAt * 1000)}${noticeIssueChip(task)}</div></div><div class="notice-row-end">${noticeDecision(task) || noticeReview(task) ? `<span class="notice-state">${noticeStatus(task)}</span>` : route.inbox_state === "archive" ? `<span class="notice-state">${noticeStatus(task)}</span>` : ""}<a class="notice-open" href="${esc(noticeRoute(task.taskID))}" aria-label="Open ${esc(task.title)}">${icon("arrow-right")}</a></div></article>`).join("") : inboxEmpty(Boolean(query || route.inbox_project))}</div></div><div class="list-footer inbox-footer"><span>${tasks.length} ${tasks.length === 1 ? "notice" : "notices"}</span><span>Answers and read receipts stay in sync.</span></div>`;
  $("#inbox-search").oninput = () => {
    const value = $("#inbox-search").value;
    clearTimeout(inboxSearchTimer);
    inboxSearchTimer = setTimeout(
      () => navigate({ inbox_search: value, notice: "" }, true),
      250,
    );
  };
  if (inboxSearchFocus) {
    $("#inbox-search").focus();
    try {
      $("#inbox-search").setSelectionRange(...inboxSearchFocus);
    } catch {}
    inboxSearchFocus = null;
  }
  $("#inbox-project-filter").onchange = () =>
    navigate({ inbox_project: $("#inbox-project-filter").value, notice: "" });
  document.title = `Inbox${unread ? ` (${unread})` : ""} · Hey Boss`;
}
async function renderInboxRoute(sequence) {
  if ($("#inbox-search") === document.activeElement)
    inboxSearchFocus = [
      $("#inbox-search").selectionStart,
      $("#inbox-search").selectionEnd,
    ];
  inboxDetail = null;
  $("#inbox-view").innerHTML =
    '<div class="loading-state"><span class="spinner"></span>Opening Inbox…</div>';
  try {
    if (model.route.notice) {
      const value = await inboxApi({
        action: "view",
        task_id: model.route.notice,
      });
      if (sequence !== model.sequence) return;
      renderNotice(value.task);
      if (
        value.task.status === "pending" &&
        !noticeDecision(value.task) &&
        !noticeReview(value.task)
      ) {
        try {
          const read = await inboxApi({
            action: "read",
            task_id: value.task.taskID,
          });
          if (sequence === model.sequence) renderNotice(read.task);
          inboxAt = 0;
          refreshInboxBadge();
        } catch (error) {
          if (sequence === model.sequence) toast(error.message, true);
        }
      }
    } else {
      await inboxSnapshot();
      if (sequence === model.sequence) renderInboxList();
    }
  } catch (error) {
    if (sequence !== model.sequence) return;
    $("#inbox-view").innerHTML =
      `<div class="empty-state"><div class="empty-icon">${icon("inbox")}</div><h1>Let’s reconnect</h1><p>${esc(error.message)}</p><button class="button" data-inbox-refresh>Try again</button><a class="button" href="${esc(routeHash({ ...model.route, view: "issues", notice: "" }))}">Go to issues</a></div>`;
  }
}
function renderNotice(task) {
  inboxDetail = task;
  document.title = `${task.title || "Notice"} · Inbox · Hey Boss`;
  const pending = task.status === "pending",
    decision = noticeDecision(task),
    review = noticeReview(task);
  const attachment =
    task.attachment &&
    ["image/png", "image/jpeg", "image/gif", "image/webp"].includes(
      task.attachment.mime,
    )
      ? `<img class="notice-attachment" src="data:${esc(task.attachment.mime)};base64,${esc(task.attachment.data)}" alt="${esc(task.attachment.name)}" />`
      : "";
  const body =
    task.body_html || '<p class="muted-text">No additional details.</p>';
  const response =
    decision && pending
      ? task.kind === "approval"
        ? `<div class="notice-choices">${task.options.map((option, index) => `<button class="button ${index === 0 ? "primary" : ""}" data-notice-answer="${esc(option)}">${esc(option)}</button>`).join("")}</div>`
        : `<form id="notice-answer-form"><label class="field-label" for="notice-answer">Your answer</label><textarea class="text-input" id="notice-answer" rows="4" required maxlength="65536" placeholder="Write your answer…">${esc(storage.get(noticeDraftKey(task, "answer")) || "")}</textarea><div class="notice-form-actions"><button class="button primary" type="submit">Send answer${icon("arrow-right")}</button></div></form>`
      : !pending
        ? `<div class="notice-outcome">${icon(task.status === "cancelled" ? "x" : "check")}<div><strong>${esc(noticeStatus(task))}</strong>${task.result ? `<p>${esc(task.result)}</p>` : ""}<span>${task.completedAt ? date(task.completedAt * 1000) : ""}</span></div></div>`
        : "";
  const comments = review
    ? `<section class="notice-comments"><h2>Review comments</h2>${(task.comments || []).map((comment) => `<article class="comment-card"><div class="comment-header"><strong>${esc(model.boss.name)}</strong><span>${date(comment.created_at * 1000)}</span></div><div class="comment-body">${comment.quote ? `<blockquote>${esc(comment.quote)}</blockquote>` : ""}<div class="markdown">${comment.body_html || esc(comment.text)}</div></div></article>`).join("")}${pending ? `<form id="notice-comment-form"><label class="field-label" for="notice-comment">Add feedback</label><textarea class="text-input" id="notice-comment" rows="3" required maxlength="16384" placeholder="Leave a review comment…">${esc(storage.get(noticeDraftKey(task, "comment")) || "")}</textarea><div class="notice-form-actions"><button class="button" data-notice-finish type="button">Finish review${icon("check")}</button><button class="button primary" type="submit">Add comment</button></div></form>` : ""}</section>`
    : "";
  $("#inbox-view").innerHTML =
    `<a class="back-link" href="${esc(noticeRoute(""))}">${icon("arrow-left")}Inbox</a><div class="detail-top"><h1>${esc(task.title || "Untitled notice")}</h1><div class="detail-heading-actions"><button class="icon-button" data-notice-copy aria-label="Copy notice link" title="Copy notice link">${icon("link")}</button></div></div><div class="detail-meta"><span class="state-pill ${pending ? "open" : "closed"}">${icon(pending ? "clock" : task.status === "cancelled" ? "x" : "check")}${esc(noticeStatus(task))}</span><span class="notice-severity-label notice-tone ${noticeSeverity(task)}">${esc(noticeLabel(task))}</span><span>${esc(task.project || "Notifications")}</span><span>·</span><span>${esc(task.sourceHost || "Source unavailable")}</span><span>·</span>${date(task.createdAt * 1000)}</div><div class="detail-layout"><div class="detail-main"><article class="comment-card notice-card notice-tone ${noticeSeverity(task)}"><div class="comment-header">${noticeBadge(task)}<strong>${esc(task.documentName || (review ? "Document review" : task.kind === "update" ? "Update" : decision ? task.question : "Notice"))}</strong></div><div class="comment-body markdown">${attachment}${body}</div></article><div class="notice-response">${response}</div>${comments}</div><section class="sidebar" aria-label="Notice properties"><div class="side-section"><h2 class="side-heading">Related issue${icon("issue")}</h2>${task.issue ? `<a class="related-issue-link" href="${esc(issueReferenceRoute(task.issue))}">${icon("issue")}<strong>Issue #${task.issue.number}</strong></a><p>${esc(task.issue.project.replace(/^named:/, ""))}${task.issue.host ? ` · ${esc(task.issue.host)}` : ""}</p>` : "<p>No issue linked</p>"}<div class="assignee-actions"><button class="button small" data-notice-link>${task.issue ? "Change link" : "Link issue"}</button>${task.issue ? '<button class="button small" data-notice-unlink>Unlink</button>' : ""}</div></div>${task.linkURL ? `<div class="side-section"><h2 class="side-heading">Destination${icon("link")}</h2><button class="button small" data-notice-open-link>${icon("arrow-right")}${esc(task.linkLabel || "Open link")}</button></div>` : ""}<div class="side-section"><h2 class="side-heading">Activity</h2><p>Received ${date(task.createdAt * 1000)}</p>${task.completedAt ? `<p>${esc(noticeStatus(task))} ${date(task.completedAt * 1000)}</p>` : ""}</div>${pending ? `<button class="button link-button ${decision || review ? "danger" : ""}" data-notice-dismiss>${icon("x")}${decision ? "Cancel question" : review ? "Cancel review" : "Mark as read"}</button>` : ""}</section></div>`;
  secureLinks();
  for (const [selector, type] of [
    ["#notice-answer", "answer"],
    ["#notice-comment", "comment"],
  ]) {
    const input = $(selector);
    if (input)
      input.oninput = () =>
        storage.set(noticeDraftKey(task, type), input.value);
  }
  if ($("#notice-answer-form"))
    $("#notice-answer-form").onsubmit = (event) => {
      event.preventDefault();
      noticeAction({ action: "respond", answer: $("#notice-answer").value });
    };
  if ($("#notice-comment-form"))
    $("#notice-comment-form").onsubmit = (event) => {
      event.preventDefault();
      noticeAction({
        action: "comment",
        body: $("#notice-comment").value,
        quote: null,
      });
    };
}
async function noticeAction(action) {
  if (inboxBusy || !inboxDetail) return;
  const task = inboxDetail,
    sequence = model.sequence;
  if (
    action.action === "dismiss" &&
    (noticeDecision(task) || noticeReview(task))
  ) {
    if (
      !(await confirmDialog(
        noticeDecision(task) ? "Cancel this question?" : "Cancel this review?",
        "The request will be cancelled. Cancellation does not count as an answer or approval.",
        "Cancel request",
      ))
    )
      return;
  }
  inboxBusy = true;
  $("#inbox-view").setAttribute("aria-busy", "true");
  $$("#inbox-view button, #inbox-view textarea").forEach(
    (el) => (el.disabled = true),
  );
  try {
    const value = await inboxApi({ ...action, task_id: task.taskID });
    inboxAt = 0;
    if (action.action === "respond" && value.changed)
      storage.remove(noticeDraftKey(task, "answer"));
    if (action.action === "comment")
      storage.remove(noticeDraftKey(task, "comment"));
    if (sequence === model.sequence) renderNotice(value.task);
    toast(
      action.action === "respond" && !value.changed
        ? "This request was already handled."
        : {
            respond: "Answer sent",
            comment: "Comment added",
            finish_review: "Review finished",
            dismiss: "Notice handled",
            link: action.issue ? "Issue linked" : "Issue unlinked",
            open_link: "Destination opened",
          }[action.action] || "Updated",
    );
    await inboxSnapshot(true);
  } catch (error) {
    toast(error.message, true);
    if (sequence === model.sequence) renderNotice(task);
  } finally {
    inboxBusy = false;
    $("#inbox-view").removeAttribute("aria-busy");
  }
}
async function refreshInbox(quiet = true) {
  if (
    inboxBusy ||
    document.hidden ||
    $("dialog[open]") ||
    $("#notice-answer")?.value.trim() ||
    $("#notice-comment")?.value.trim() ||
    $("#inbox-search") === document.activeElement
  )
    return;
  const sequence = model.sequence;
  try {
    await inboxSnapshot(true);
    if (sequence !== model.sequence) return;
    if (model.route.notice) {
      const value = await inboxApi({
        action: "view",
        task_id: model.route.notice,
      });
      if (
        sequence === model.sequence &&
        JSON.stringify(value.task) !== JSON.stringify(inboxDetail)
      )
        renderNotice(value.task);
    } else {
      const query = model.route.inbox_search.trim().toLocaleLowerCase(),
        tasks = inboxTasks.filter(
          (task) =>
            (model.route.inbox_state === "archive"
              ? task.status !== "pending"
              : task.status === "pending") &&
            (!model.route.inbox_project ||
              task.project === model.route.inbox_project) &&
            (!query ||
              `${task.title} ${task.summary} ${task.project} ${task.sourceHost}`
                .toLocaleLowerCase()
                .includes(query)),
        );
      if (
        JSON.stringify([
          tasks,
          model.route.inbox_state,
          model.route.inbox_project,
          model.route.inbox_search,
        ]) !== inboxSignature
      )
        renderInboxList();
    }
    if (!quiet) toast("Inbox refreshed");
  } catch (error) {
    if (!quiet) toast(error.message, true);
  }
}
async function loadRelatedNotices(number, project, host, snapshot = null) {
  const sequence = model.sequence;
  const current = () =>
    sequence === model.sequence &&
    model.route.view === "issues" &&
    model.route.issue === number &&
    model.project?.id === project &&
    (model.route.host || "") === (host || "");
  const heading = `<h2 class="side-heading">Related notices${icon("inbox")}</h2>`;
  try {
    const tasks = snapshot || (await inboxSnapshot());
    if (!current()) return;
    const related = tasks.filter(
      (task) =>
        task.issue?.number === number &&
        task.issue.project === project &&
        (task.issue.host || "") === (host || ""),
    );
    const root = $("#related-notices");
    if (!root) return;
    root.hidden = false;
    const signature = JSON.stringify(
      related.map((task) => [task.taskID, task.title, noticeStatus(task)]),
    );
    if (root.relatedSignature === signature) return;
    root.relatedSignature = signature;
    const focused = root.contains(document.activeElement)
      ? document.activeElement.getAttribute("href")
      : null;
    root.innerHTML = `${heading}${related.length ? related.map((task) => `<a class="related-notice-link" href="${esc(noticeRoute(task.taskID))}"><span>${esc(task.title)}</span><small>${esc(noticeStatus(task))}</small></a>`).join("") : "<p>No linked notices</p>"}`;
    if (focused) {
      const target = $$("a", root).find(
        (link) => link.getAttribute("href") === focused,
      );
      const fallback = $("h2", root);
      if (!target && fallback) fallback.tabIndex = -1;
      (target || fallback)?.focus({ preventScroll: true });
    }
  } catch {
    if (!current()) return;
    const root = $("#related-notices");
    if (root && !root.querySelector(".related-notice-link")) {
      root.hidden = false;
      delete root.relatedSignature;
      root.innerHTML = `${heading}<p>Notices unavailable</p>`;
    }
  }
}
async function openNoticeLink(button) {
  noticeLinkTask = inboxDetail?.taskID;
  noticeLinkFocus = button;
  noticeLinkSelection = null;
  $("#notice-link-search").value = "";
  $("#notice-link-host").value =
    inboxDetail?.issue?.host || model.route.host || "";
  $("#notice-link-error").hidden = true;
  $("#notice-link-submit").disabled = true;
  $("#notice-link-dialog").showModal();
  await loadNoticeLinkProjects();
}
function closeNoticeLink() {
  if (inboxBusy) return;
  ++noticeLinkSequence;
  $("#notice-link-dialog").close();
  noticeLinkFocus?.focus();
}
async function loadNoticeLinkProjects() {
  const sequence = ++noticeLinkSequence,
    host = $("#notice-link-host").value.trim() || null;
  $("#notice-link-state").textContent = "Loading projects…";
  $("#notice-link-results").innerHTML = "";
  $("#notice-link-submit").disabled = true;
  try {
    const value = await api(
      { action: "projects", include_hidden: true },
      model.project.id,
      null,
      host,
    );
    if (sequence !== noticeLinkSequence) return;
    $("#notice-link-project").innerHTML = value.projects
      .map(
        (project) =>
          `<option value="${esc(project.id)}">${esc(project.name)}</option>`,
      )
      .join("");
    const matches = value.projects.filter(
      (project) =>
        project.id === inboxDetail?.project ||
        project.name === inboxDetail?.project,
    );
    const preferred =
      inboxDetail?.issue?.project ||
      (matches.length === 1 ? matches[0].id : model.project.id);
    if (value.projects.some((project) => project.id === preferred))
      $("#notice-link-project").value = preferred;
    await loadNoticeLinkIssues();
  } catch (error) {
    if (sequence === noticeLinkSequence) noticeLinkError(error);
  }
}
async function loadNoticeLinkIssues() {
  const sequence = ++noticeLinkSequence,
    project = $("#notice-link-project").value,
    host = $("#notice-link-host").value.trim() || null;
  noticeLinkSelection = null;
  $("#notice-link-submit").disabled = true;
  $("#notice-link-state").textContent = "Loading issues…";
  try {
    const value = await api(
      {
        action: "list",
        state: "all",
        mine: false,
        unassigned: false,
        assignee: null,
        labels: [],
        search: null,
        limit: 50,
        offset: 0,
        all: true,
      },
      project,
      null,
      host,
    );
    if (sequence !== noticeLinkSequence) return;
    noticeLinkIssues = value.issues;
    $("#notice-link-state").textContent = "";
    $("#notice-link-error").hidden = true;
    renderNoticeLinkIssues();
  } catch (error) {
    if (sequence === noticeLinkSequence) noticeLinkError(error);
  }
}
function noticeLinkError(error) {
  $("#notice-link-state").textContent = "";
  $("#notice-link-error").textContent = error.message;
  $("#notice-link-error").hidden = false;
}
function renderNoticeLinkIssues() {
  const query = $("#notice-link-search").value.trim().toLocaleLowerCase(),
    issues = noticeLinkIssues.filter((issue) =>
      `${issue.number} ${issue.title}`.toLocaleLowerCase().includes(query),
    );
  $("#notice-link-results").innerHTML = issues.length
    ? issues
        .map(
          (issue) =>
            `<label class="notice-link-option"><input type="radio" name="related_issue" value="${issue.number}" ${noticeLinkSelection === issue.number ? "checked" : ""}/>${icon(issue.state === "closed" ? "closed" : "issue")}<span><strong>#${issue.number} ${esc(issue.title)}</strong><small>${esc(issue.state)}</small></span></label>`,
        )
        .join("")
    : '<p class="field-help">No matching issues</p>';
}
function initInbox() {
  if ($("#inbox-view").dataset.initialized) return;
  $("#inbox-view").dataset.initialized = "true";
  $("#inbox-view").onclick = async (event) => {
    const button = event.target.closest("button");
    if (!button) return;
    if (button.hasAttribute("data-inbox-refresh")) {
      inboxAt = 0;
      await renderRoute();
      return;
    }
    if (button.hasAttribute("data-inbox-clear")) {
      navigate({ inbox_project: "", inbox_search: "" });
      return;
    }
    if (button.dataset.inboxState) {
      navigate({ inbox_state: button.dataset.inboxState, notice: "" });
      return;
    }
    if (button.hasAttribute("data-notice-link")) {
      openNoticeLink(button);
      return;
    }
    if (button.hasAttribute("data-notice-unlink")) {
      noticeAction({ action: "link", issue: null });
      return;
    }
    if (button.hasAttribute("data-notice-answer")) {
      noticeAction({ action: "respond", answer: button.dataset.noticeAnswer });
      return;
    }
    if (button.hasAttribute("data-notice-dismiss")) {
      noticeAction({ action: "dismiss" });
      return;
    }
    if (button.hasAttribute("data-notice-finish")) {
      noticeAction({ action: "finish_review" });
      return;
    }
    if (button.hasAttribute("data-notice-open-link")) {
      noticeAction({ action: "open_link" });
      return;
    }
    if (button.hasAttribute("data-notice-copy")) {
      try {
        await navigator.clipboard.writeText(location.href);
        toast("Notice link copied");
      } catch {
        toast("Copy the URL from your address bar.");
      }
    }
  };
  for (const selector of ["#notice-link-close", "#notice-link-cancel"])
    $(selector).onclick = closeNoticeLink;
  $("#notice-link-dialog").addEventListener("cancel", (event) => {
    event.preventDefault();
    closeNoticeLink();
  });
  $("#notice-link-host").onchange = loadNoticeLinkProjects;
  $("#notice-link-project").onchange = loadNoticeLinkIssues;
  $("#notice-link-search").oninput = renderNoticeLinkIssues;
  $("#notice-link-results").onchange = (event) => {
    noticeLinkSelection = Number(event.target.value);
    $("#notice-link-submit").disabled = !noticeLinkSelection;
  };
  $("#notice-link-form").onsubmit = async (event) => {
    event.preventDefault();
    if (!noticeLinkSelection || inboxBusy) return;
    const task = noticeLinkTask,
      issue = {
        project: $("#notice-link-project").value,
        number: noticeLinkSelection,
        host: $("#notice-link-host").value.trim() || null,
      };
    $("#notice-link-submit").disabled = true;
    $("#notice-link-state").textContent = "Linking…";
    inboxBusy = true;
    try {
      const value = await inboxApi({ action: "link", task_id: task, issue });
      inboxAt = 0;
      inboxBusy = false;
      closeNoticeLink();
      if (inboxDetail?.taskID === task) renderNotice(value.task);
      toast("Issue linked");
      refreshInboxBadge();
    } catch (error) {
      noticeLinkError(error);
      $("#notice-link-submit").disabled = false;
    } finally {
      inboxBusy = false;
    }
  };
}
