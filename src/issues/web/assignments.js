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
  function describe(issue, {actorName, bossName}) {
    const a = current(issue), machine = a.machine_name || a.machine;
    if (a.kind === "github") return {label:"GitHub watcher", detail:issue.state === "closed" || issue.deleted_at ? "Monitoring stopped for this issue." : a.actor ? `${actorName(a.actor)} is working${machine ? ` on ${machine}` : ""}.` : a.waiting ? "Waiting for required failures or all checks to finish." : "New GitHub findings are queued for an agent.", icon:"pull-request"};
    if (a.kind === "machine") return {label:machine || "Machine", detail:"Waiting for an agent.", icon:"monitor"};
    if (a.kind === "agent") return {label:actorName(a.actor), detail:machine ? `Working on ${machine}.` : "Agent is working.", icon:"user"};
    if (a.kind === "boss") return {label:bossName, detail:"Assigned to you.", icon:"user"};
    return {label:"Unassigned", detail:"An available machine can pick this up.", icon:"user"};
  }
  function render(value, helpers) {
    const issue = value.issue, a = current(issue), description = describe(issue, helpers);
    const editable = ["open", "ready"].includes(issue.state) && !issue.deleted_at && !issue.draft;
    const hasPr = issue.pull_requests?.some(watchablePr);
    const selected = a.kind === "github" ? "github" : a.kind === "boss" ? "boss" : a.kind === "machine" ? `machine:${a.machine}` : a.kind === "agent" ? "active" : "unassigned";
    const targets = [{id:"unassigned", name:"Unassigned"}, {id:"boss", name:helpers.bossName}, {id:"github", name:"GitHub watcher", disabled:!hasPr}];
    for (const machine of value.assignment_machines || []) targets.push({id:`machine:${machine.id}`, name:machine.name || machine.host || machine.id});
    if (a.kind === "machine" && !targets.some(t => t.id === selected)) targets.push({id:selected, name:description.label});
    if (a.kind === "agent") targets.unshift({id:"active", name:description.label, disabled:true});
    const options = targets.map(t => `<option value="${esc(t.id)}"${t.id === selected ? " selected" : ""}${t.disabled ? " disabled" : ""}>${esc(t.name)}</option>`).join("");
    return `<div class="side-section issue-assignment"><label class="side-heading" for="issue-assignment">Assignment${helpers.icon(description.icon)}</label><select id="issue-assignment" data-assignment-select aria-describedby="assignment-detail"${editable ? "" : " disabled"}>${options}</select><p id="assignment-detail" class="assignment-detail">${esc(issue.draft ? "Mark ready before assigning this issue." : description.detail)}</p>${editable && !hasPr ? '<p class="assignment-hint">Attach a PR to enable the GitHub watcher.</p>' : ""}</div>`;
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
  const api = {current, describe, render, status};
  if (typeof module !== "undefined") module.exports = api;
  else root.IssueAssignments = api;
})(globalThis);
