"use strict";
(function(root) {
  const esc = value => String(value ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
  const safeUrl = value => {
    try { const url = new URL(value); return ["https:", "http:"].includes(url.protocol) ? url.href : null; }
    catch { return null; }
  };
  const link = (url, text) => safeUrl(url) ? `<a href="${esc(safeUrl(url))}" target="_blank" rel="noopener noreferrer">${esc(text)}</a>` : esc(text);
  const watchablePr = pr => {
    if (["closed", "merged"].includes(pr.status)) return false;
    const match = /^https:\/\/github\.com\/([\w.-]+)\/([\w.-]+)\/pull\/([1-9][0-9]{0,19})\/?$/.exec(pr.url);
    return !!match && ![match[1], match[2]].some(part => part === "." || part === "..") && (match[3].length < 20 || match[3] <= "18446744073709551615");
  };
  function current(issue) {
    return issue.assignment || (issue.assignee === "watcher:github" ? {kind:"github", waiting:true} : issue.assignee === "human:boss" ? {kind:"boss"} : issue.assignee ? {kind:"agent", actor:issue.assignee} : {kind:"unassigned"});
  }
  function describeAssignment(issue, {actorName, bossName}) {
    const a = current(issue), machine = a.machine_name || a.machine;
    if (a.kind === "github") return {label:"GitHub watcher", detail:issue.state === "closed" || issue.deleted_at ? "Monitoring stopped for this issue." : a.actor ? `${actorName(a.actor)} is working${machine ? ` on ${machine}` : ""}.` : a.waiting ? "Waiting for required failures or all checks to finish." : "New GitHub findings are queued for an agent.", icon:"pull-request"};
    if (a.kind === "machine") return {label:machine || "Machine", detail:"Waiting for an agent.", icon:"monitor"};
    if (a.kind === "agent") return {label:actorName(a.actor), detail:machine ? `Working on ${machine}.` : "Agent is working.", icon:"user"};
    if (a.kind === "boss") return {label:bossName, detail:"Assigned to you.", icon:"user"};
    return {label:"Unassigned", detail:"An available machine can pick this up.", icon:"user"};
  }
  function describe(issue, helpers) {
    const description = describeAssignment(issue, helpers);
    if (issue.attempt_hold) description.detail = "Pickup is paused until the retained attempt is reconciled.";
    return description;
  }
  function render(value, helpers) {
    const issue = value.issue, a = current(issue), description = describe(issue, helpers);
    const editable = ["open", "ready"].includes(issue.state) && !issue.deleted_at && !issue.draft && !issue.attempt_hold;
    const hasPr = issue.pull_requests?.some(watchablePr);
    const selected = a.kind === "github" ? "github" : a.kind === "boss" ? "boss" : a.kind === "machine" ? `machine:${a.machine}` : a.kind === "agent" ? "active" : "unassigned";
    const targets = [{id:"unassigned", name:"Unassigned"}, {id:"boss", name:helpers.bossName}, {id:"github", name:"GitHub watcher", disabled:!hasPr}];
    for (const machine of value.assignment_machines || []) targets.push({id:`machine:${machine.id}`, name:machine.name || machine.host || machine.id});
    if (a.kind === "machine" && !targets.some(t => t.id === selected)) targets.push({id:selected, name:description.label});
    if (a.kind === "agent") targets.unshift({id:"active", name:description.label, disabled:true});
    const options = targets.map(t => `<option value="${esc(t.id)}"${t.id === selected ? " selected" : ""}${t.disabled ? " disabled" : ""}>${esc(t.name)}</option>`).join("");
    return `<div class="side-section issue-assignment"><label class="side-heading" for="issue-assignment">Assignment${helpers.icon(description.icon)}</label><select id="issue-assignment" data-assignment-select aria-describedby="assignment-detail"${editable ? "" : " disabled"}>${options}</select><p id="assignment-detail" class="assignment-detail">${esc(issue.draft ? "Mark ready before assigning this issue." : description.detail)}</p>${fetchOverview(issue)}${editable && !hasPr ? '<p class="assignment-hint">Attach a PR to enable the GitHub watcher.</p>' : ""}</div>`;
  }
  function fetchTime(value) {
    const date = typeof value === "number" && value > 0 ? new Date(value) : null;
    return date && Number.isFinite(date.getTime()) ? `<time datetime="${date.toISOString()}" data-absolute title="${esc(date.toLocaleString())}">${esc(date.toLocaleString(undefined, {month:"short",day:"numeric",hour:"numeric",minute:"2-digit",second:"2-digit"}))}</time>` : "Not recorded yet";
  }
  function fetchOverview(issue) {
    if (current(issue).kind !== "github") return "";
    const watch = issue.github_status || {}, now = Date.now();
    const active = watch.monitoring !== false && issue.state !== "closed" && !issue.deleted_at && !issue.draft;
    const prs = (issue.pull_requests || []).filter(watchablePr);
    const rows = prs.map(pr => {
      const f = watch.fetches?.[pr.url] || {}, snapshot = watch.prs?.[pr.url] || {};
      const fetching = f.started_at > (f.finished_at || 0) && f.started_at > now - 120000;
      const interrupted = f.started_at > (f.finished_at || 0) && !fetching;
      const queued = f.requested_at > (f.started_at || 0) || !!f.requested_at && !fetching;
      const error = f.error || snapshot.error;
      const delayed = f.next_at > now;
      const label = !active ? "Monitoring stopped" : fetching ? "Fetching GitHub…" : queued ? "Fetch queued" : interrupted ? "Fetch interrupted; waiting to retry" : error ? "Fetch needs retry" : !f.finished_at ? "Waiting for first fetch" : "Watching checks and reviews";
      return {busy:fetching || queued, html:`<div class="github-fetch"><p class="github-fetch-state" role="status">${prs.length > 1 ? link(pr.url, "PR #" + pr.url.split("/").filter(Boolean).pop()) + " · " : ""}${esc(label)}</p><p class="assignment-hint">Last fetch ${fetchTime(f.finished_at)}</p>${active && delayed ? `<p class="assignment-hint">${error || queued ? "Retry after" : "Next poll after"} ${fetchTime(f.next_at)}</p>` : ""}${error ? `<p class="github-status-error">${esc(error)}</p>` : ""}</div>`};
    });
    return `<div class="github-fetch-overview">${rows.map(row => row.html).join("")}${active && prs.length ? `<button type="button" class="github-fetch-now" data-action="refresh_github"${rows.some(row => row.busy) ? " disabled" : ""}>Fetch now</button><p class="assignment-hint">Fetches on the next watcher cycle, subject to GitHub rate limits.</p>` : ""}</div>`;
  }
  const checkState = state => ({failure:"Failed",satisfied:"Passed",success:"Passed",pending:"Running",missing:"Not reported",unknown:"Unknown",not_required:"No required checks"}[state] || state || "Unknown");
  function status(issue, helpers) {
    const watch = issue.github_status;
    if (!watch && current(issue).kind !== "github") return "";
    const prs = Object.entries(watch?.prs || {}).sort(([a],[b]) => Number(b === watch?.trigger?.url) - Number(a === watch?.trigger?.url));
    const omittedPrs = Number.isSafeInteger(watch?.omitted_prs) && watch.omitted_prs > 0 ? `<p class="assignment-hint">${watch.omitted_prs} additional pull requests omitted from this summary. See the linked PRs for full details.</p>` : "";
    const paused = watch?.monitoring === false ? `<p class="assignment-hint">${watch.stopped_reason === "no_open_pull_requests" ? "No open GitHub pull requests remain." : "Monitoring paused. Last recorded status:"}</p>` : "";
    const error = watch?.error ? `<p class="github-status-error" role="status"><strong>Status unavailable</strong><br>${esc(watch.error)}</p>` : "";
    const entries = prs.map(([url, snapshot]) => {
      const evidence = snapshot.evidence || {}, required = evidence.required || [];
      const failed = required.filter(c => c.state === "failure");
      const failureCount = Number.isSafeInteger(evidence.required_counts?.failure) ? Math.max(failed.length, evidence.required_counts.failure) : failed.length;
      const sourceNames = ["source_errors", "ci_errors", "policy_errors"];
      const incomplete = sourceNames.some(name => evidence[name]?.length || evidence.omitted?.[name] > 0);
      const policyIncomplete = evidence.policy_errors?.length || evidence.omitted?.policy_errors > 0;
      const messages = [...new Set(sourceNames.flatMap(name => (evidence[name] || []).map(error => typeof error === "string" ? error : error.message)).filter(message => message && message !== snapshot.error))];
      const sourceErrors = messages.slice(0, 3).map(message => `<p class="github-status-error" role="status">${esc(message)}</p>`).join("");
      const changed = evidence.sources_match === false;
      const terminal = {closed:"Pull request closed without merging", merged:"Pull request merged", removed:"Pull request removed from this issue"}[snapshot.lifecycle];
      const label = terminal || (changed ? "Refreshing changed pull request" : failureCount ? `${failureCount} required check${failureCount === 1 ? "" : "s"} failed${snapshot.error ? " (last recorded)" : policyIncomplete ? " (policy incomplete)" : ""}` : snapshot.error ? "Status unavailable" : incomplete ? "Status incomplete" : evidence.ci_settled && evidence.has_checks === false ? "No checks reported" : evidence.complete ? "All checks finished" : "Checks in progress");
      const checks = required.map(c => `<li><span class="github-check-state ${c.state === "failure" ? "failed" : ""}">${esc(checkState(c.state))}</span>${link(c.url, c.context)}</li>`).join("");
      const feedback = [...(evidence.reviews || []).filter(r => r.body), ...(evidence.comments || []).filter(c => c.body), ...(evidence.review_comments || []).filter(c => c.body)].slice(0, 5);
      const reviews = feedback.length ? `<details class="github-feedback"><summary>Recent review feedback</summary>${feedback.map(r => `<article><p>${esc(r.body)}</p>${r.html_url ? link(r.html_url, "View on GitHub") : ""}</article>`).join("")}</details>` : "";
      const omittedNames = {checks:"checks",required:"required checks",statuses:"status checks",workflows:"workflows",failures:"failure details",reviews:"reviews",review_comments:"inline comments",comments:"comments",source_errors:"source errors",ci_errors:"CI errors",policy_errors:"policy errors"};
      const omitted = Object.entries(evidence.omitted || {}).filter(([,count]) => Number.isSafeInteger(count) && count > 0).map(([name,count]) => `${count} ${omittedNames[name] || "results"}`).join(", ");
      const summary = evidence.truncated ? `<p class="assignment-hint">Summary shown${omitted ? `; ${esc(omitted)} omitted` : ""}. View the PR on GitHub for full details.</p>` : "";
      const date = typeof snapshot.checked_at === "number" && Number.isFinite(snapshot.checked_at) && snapshot.checked_at > 0 ? new Date(snapshot.checked_at) : null;
      const observed = date && Number.isFinite(date.getTime()) ? `<p class="github-observed">Observed <time datetime="${date.toISOString()}">${esc(date.toLocaleString(undefined, {month:"short",day:"numeric",hour:"numeric",minute:"2-digit"}))}</time></p>` : "";
      return `<article class="github-pr-status"><h4>${link(url, evidence.repository && evidence.number ? `${evidence.repository} #${evidence.number}` : "Pull request")}</h4><p class="github-check-summary${failed.length && !terminal && !changed ? " failed" : ""}">${esc(label)}</p>${snapshot.error ? `<p class="github-status-error" role="status">${esc(snapshot.error)}</p>` : ""}${sourceErrors}${checks ? `<details><summary>${changed ? "Last recorded required checks" : "Required checks"}</summary><ul class="github-checks">${checks}</ul></details>` : ""}${reviews}${summary}${observed}</article>`;
    }).join("");
    return `<section class="side-section github-watch-status" aria-label="GitHub status"><h3 class="side-heading">GitHub status${helpers.icon("pull-request")}</h3>${paused}${error}${entries || (!error && !paused ? '<p class="assignment-detail">Waiting for the first GitHub status.</p>' : "")}${omittedPrs}</section>`;
  }
  function initWatcher({list, context, read, refresh, actorName, bossName, icon}) {
    if (document.querySelector(".github-watcher-dialog")) return;
    const dialog = document.createElement("dialog");
    dialog.className = "github-watcher-dialog";
    dialog.setAttribute("aria-labelledby", "github-watcher-title");
    dialog.innerHTML = `<header class="dialog-heading"><h2 id="github-watcher-title">GitHub watcher</h2><button type="button" class="button" data-close aria-label="Close watcher">Close</button></header><div class="dialog-content"><a data-issue-link></a><div data-watcher-content></div><p class="github-status-error" data-error role="status" hidden></p></div>`;
    document.body.append(dialog);
    const content = dialog.querySelector("[data-watcher-content]"), error = dialog.querySelector("[data-error]");
    let active = null;
    const valid = ctx => active === ctx && dialog.open;
    function renderWatcher(ctx, issue) {
      const description = describe(issue, {actorName, bossName:bossName()});
      const actor = current(issue).actor;
      const trace = actor ? '/agents/session#' + new URLSearchParams({project:ctx.project, issue:ctx.number, agent:actor}) : null;
      const html = `<p class="assignment-detail">${esc(description.detail)}</p>${trace ? `<p><a href="${esc(trace)}">Open agent conversation</a></p>` : ""}${fetchOverview(issue)}${status(issue, {icon})}`;
      if (html === ctx.html) return;
      const detailKey = el => `${el.closest("article")?.querySelector("h4 a")?.href}:${el.querySelector("summary")?.textContent}`;
      const expanded = new Set([...content.querySelectorAll("details[open]")].map(detailKey));
      const fetchFocused = content.querySelector(".github-fetch-now") === document.activeElement;
      content.innerHTML = html;
      ctx.html = html;
      content.querySelectorAll("details").forEach(el => { el.open = expanded.has(detailKey(el)); });
      if (fetchFocused) content.querySelector(".github-fetch-now:not(:disabled)")?.focus();
    }
    async function update(ctx, force = false) {
      if (ctx.pending) {
        if (!force) return;
        await ctx.pending;
      }
      if (!valid(ctx)) return;
      clearTimeout(ctx.timer);
      if (!force && document.hidden) {
        ctx.timer = setTimeout(() => update(ctx), 5000);
        return;
      }
      error.hidden = true;
      if (force) content.querySelector(".github-fetch-now")?.setAttribute("disabled", "");
      ctx.pending = (async () => {
        try {
          const result = await (force ? refresh(ctx) : read(ctx));
          if (valid(ctx)) {
            // Force a render after a manually disabled button, even on an unchanged response.
            if (force) ctx.html = null;
            renderWatcher(ctx, result.issue);
          }
        } catch (failure) {
          if (valid(ctx)) {
            error.textContent = failure.message || "Unable to load GitHub watcher status.";
            error.hidden = false;
            if (force) content.querySelector(".github-fetch-now")?.removeAttribute("disabled");
          }
        }
      })();
      await ctx.pending;
      ctx.pending = null;
      if (valid(ctx)) ctx.timer = setTimeout(() => update(ctx), 5000);
    }
    function close() {
      if (!active) return;
      const ctx = active;
      active = null;
      clearTimeout(ctx.timer);
      if (dialog.open) dialog.close();
      const trigger = ctx.trigger.isConnected ? ctx.trigger : list.querySelector(`[data-open-watcher="${ctx.number}"]`);
      trigger?.focus();
    }
    dialog.querySelector("[data-close]").onclick = close;
    dialog.addEventListener("cancel", event => { event.preventDefault(); close(); });
    dialog.addEventListener("close", () => { if (!dialog.open) close(); });
    window.addEventListener("hashchange", close);
    content.addEventListener("click", event => {
      const button = event.target.closest('[data-action="refresh_github"]');
      if (!button || button.disabled || !active) return;
      button.disabled = true;
      update(active, true);
    });
    list.addEventListener("click", event => {
      const trigger = event.target.closest("[data-open-watcher]");
      if (!trigger) return;
      const number = Number(trigger.dataset.openWatcher);
      close();
      const ctx = {...context(number), number, trigger};
      active = ctx;
      const link = dialog.querySelector("[data-issue-link]");
      link.href = ctx.issueUrl;
      link.textContent = `Issue #${number}`;
      content.innerHTML = '<p class="assignment-detail" role="status">Loading watcher activity…</p>';
      error.hidden = true;
      dialog.showModal();
      update(ctx);
    });
  }
  const api = {current, describe, render, status, fetchOverview, initWatcher};
  if (typeof module !== "undefined") module.exports = api;
  else root.IssueAssignments = api;
})(globalThis);
