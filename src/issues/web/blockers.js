"use strict";
const IssueBlockers = (() => {
  function kind(issue) {
    if (issue.attempt_hold && !issue.deleted_at) return "hold";
    if (issue.deleted_at || issue.state !== "blocked") return "";
    return issue.manual_blocked ? "hold" : "dependencies";
  }
  function matches(issue, filter) {
    if (!filter) return true;
    if (!kind(issue)) return false;
    return filter === "hold" ? !!issue.manual_blocked || !!issue.attempt_hold : !!issue.blocked_by?.length;
  }
  const description = issue => issue.attempt_hold ? "Task attempt protected" : kind(issue) === "hold"
    ? `On hold${issue.blocked_by?.length ? " · also waiting for dependencies" : ""}`
    : "Waiting for dependencies";
  let context = null, generation = 0, busy = false;
  const href = number => routeHash({...model.route, view:"issues", issue:number});
  const link = i => `<a class="issue-blocker-link" data-issue="${i.number}" href="${esc(href(i.number))}" title="${esc(i.title)}">#${i.number} ${esc(i.title)}</a>`;
  const prBadge = i => {
    const prs = i.pull_requests || [];
    if (!prs.length) return "";
    const pr = prs[0];
    const num = (pr.url || "").match(/\/pull\/(\d+)/)?.[1];
    return `<a class="dep-pr-badge" href="${esc(pr.url)}" target="_blank" rel="noopener" title="Attached PR ${esc(pr.url)}">${icon("pull-request")}${num ? `PR #${num}` : "PR"}</a>`;
  };
  function list(issue) {
    const active = issue.blocked_by || [];
    const blocking = (issue.blocking || []).filter(d => d.actively_blocked);
    const satisfiedWithPrs = (issue.blocker_links || []).filter(i => i.satisfied && i.pull_requests?.length);
    const hold = kind(issue) === "hold" ? '<span class="issue-hold-label">On hold</span>' : "";
    const blockedPart = active.length
      ? `<span class="issue-blocked-by">${icon("blocked")}<span>Blocked by</span>${active.slice(0,3).map(link).join("")}${active.length>3 ? `<a data-issue="${issue.number}" href="${esc(href(issue.number))}">+${active.length-3} more</a>` : ""}</span>`
      : "";
    const stackedPart = !active.length && satisfiedWithPrs.length
      ? `<span class="issue-blocked-by issue-stacked-on">${icon("pull-request")}<span>Stacked on</span>${satisfiedWithPrs.slice(0,2).map(link).join("")}</span>`
      : "";
    const blockingPart = blocking.length
      ? `<span class="issue-blocked-by issue-blocking-others"><span>Blocks</span>${blocking.slice(0,2).map(link).join("")}${blocking.length>2 ? `<a data-issue="${issue.number}" href="${esc(href(issue.number))}">+${blocking.length-2} more</a>` : ""}</span>`
      : "";
    return `${hold}${blockedPart}${stackedPart}${blockingPart}`;
  }
  function card(issue) {
    if (issue.deleted_at || issue.state==="closed") return "";
    const active = issue.blocked_by || [], linked = issue.blocker_links || [], dependents = issue.blocking || [];
    const linkedNumbers = new Set(linked.map(i=>i.number));
    const inherited = active.filter(i=>!linkedNumbers.has(i.number));
    const prerequisites = [...linked, ...inherited];
    const hasGraph = prerequisites.length > 0 || dependents.length > 0;
    const stateBadge = i => {
      const label = i.deleted_at ? "Deleted" : i.state==="ready" ? "Ready" : i.state==="closed" ? "Closed" : i.state==="blocked" ? "Blocked" : i.source==="subtask" ? "Unfinished subtask" : "Open";
      return `<span class="dep-state-pill is-${esc(i.state || "open")}">${label}</span>`;
    };
    const graphHtml = hasGraph ? `<div class="dep-chain-graph" aria-label="Dependency chain visualization">
      ${prerequisites.length ? `<div class="dep-chain-stage"><div class="dep-stage-label">Prerequisites (${active.length ? "Blocked by" : "Depends on"})</div><ul class="blocker-list">${prerequisites.map(i=>`<li class="dep-node ${i.state==="ready"||i.state==="closed" ? "is-satisfied" : "is-waiting"}">${link(i)}${prBadge(i)}${stateBadge(i)}${linkedNumbers.has(i.number) ? `<button type="button" class="icon-button" data-remove-blocker="${i.number}" aria-label="Remove blocker #${i.number}">${icon("x")}</button>` : ""}</li>`).join("")}</ul></div><div class="dep-flow-arrow" aria-hidden="true">↓ unblocks</div>` : ""}
      <div class="dep-chain-current"><span class="dep-current-badge">#${issue.number} ${esc(issue.title)}</span>${prBadge(issue)}<span class="dep-state-pill is-${esc(issue.state)}">${esc(issue.state)}</span></div>
      ${dependents.length ? `<div class="dep-flow-arrow" aria-hidden="true">↓ blocks (${dependents.length})</div><div class="dep-chain-stage"><div class="dep-stage-label">Blocking downstream tasks</div><ul class="blocker-list">${dependents.map(d=>`<li class="dep-node ${d.actively_blocked ? "is-waiting" : "is-satisfied"}">${link(d)}${prBadge(d)}${stateBadge(d)}${d.unblocks_on_release ? '<span class="dep-unblock-hint">Unblocks on Ready/Close</span>' : ""}</li>`).join("")}</ul></div>` : ""}
    </div>` : `<p>${issue.state==="blocked" ? "No linked issue. Review the blocker in comments." : "No blocking issues."}</p>`;
    return `<div class="side-section issue-blockers"><h2 class="side-heading">Dependencies & Stack${icon("blocked")}</h2>${graphHtml}<div class="dep-actions-row"><button type="button" class="button" data-add-blocker>${icon("plus")}Add blocker</button><button type="button" class="button" data-create-dependent="${issue.number}">${icon("plus")}Create dependent</button></div>${active.length ? `<p>${issue.draft && issue.state === "open" ? "These links are kept while you draft. Mark ready to check them again." : issue.manual_blocked ? "Release the hold once it is resolved. These dependencies will still pause pickup." : issue.dependency_ready_state === "ready" ? "Reopens automatically when blockers are Ready or Closed. Stack your PR on the prerequisite PR branches." : "Reopens automatically when all blocking issues are Closed."}</p>` : ""}</div>`;
  }
  function close() { if(busy)return;generation++;context=null;$("#blocker-picker-dialog").close();$("[data-add-blocker]")?.focus(); }
  function options() {
    if(!context)return;
    const search=$("#blocker-picker-search").value.trim().toLowerCase();
    const existing=new Set((context.issue.blocker_numbers||[]));
    const choices=context.issues.filter(i=>i.number!==context.issue.number && !i.deleted_at && i.state!=="closed" && !existing.has(i.number) && (!search || `${i.number} ${i.title}`.toLowerCase().includes(search)));
    $("#blocker-picker-results").innerHTML=choices.length ? choices.map(i=>`<button type="button" class="subtask-option" data-select-blocker="${i.number}" ${busy?"disabled":""}><span>#${i.number}</span><strong>${esc(i.title)}</strong><span>${i.state==="ready"?"Ready":i.state==="blocked"?"Blocked":"Open"}</span></button>`).join("") : '<p class="muted-text">No matching unfinished issues.</p>';
  }
  async function open() {
    if(busy)return; const token=++generation;
    context={issue:model.detail.issue,project:model.project.id,host:model.route.host,issues:[]};
    $("#blocker-picker-search").value="";$("#blocker-picker-error").hidden=true;$("#blocker-picker-results").innerHTML='<p class="muted-text">Loading issues…</p>';
    $("#blocker-picker-dialog").showModal();$("#blocker-picker-search").focus();
    try { const value=await api({action:"list",state:"all",all:true,limit:100,offset:0,mine:false,unassigned:false,labels:[],search:null},context.project,null,context.host);if(token!==generation)return;context.issues=value.issues;options(); }
    catch(e){if(token!==generation)return;$("#blocker-picker-error").textContent=e.message;$("#blocker-picker-error").hidden=false;}
  }
  async function change(issue, numbers, project, host) {
    let force=false;
    if(issue.assignee && !own(issue.assignee)) {
      force=await confirmDialog("Block another session’s issue?",`${actorName(issue.assignee)} owns this issue. Adding blockers pauses work and clears its claim.`,"Continue");
      if(!force)return false;
    }
    await mutate({action:"set_blockers",number:issue.number,blockers:numbers,if_version:issue.version,force},project,host);
    return true;
  }
  async function add(number) {
    if(!context||busy)return;busy=true;const current=context;options();$("#blocker-picker-error").hidden=true;
    try {saveComment();const changed=await change(current.issue,[...(current.issue.blocker_numbers||[]),number],current.project,current.host);busy=false;if(!changed)return;close();await renderRoute();toast("Blocker added");}
    catch(e){if(context!==current)return;$("#blocker-picker-error").textContent=e.message;$("#blocker-picker-error").hidden=false;if(e.code==="conflict"){try{const value=await api({action:"view",number:current.issue.number},current.project,null,current.host);current.issue=value.issue;}catch{}}}
    finally{busy=false;options();}
  }
  async function remove(number,button) {
    if(busy)return;busy=true;button.disabled=true;const issue=model.detail.issue;
    try{saveComment();if(await change(issue,(issue.blocker_numbers||[]).filter(n=>n!==number),model.project.id,model.route.host)){await renderRoute();toast("Blocker removed");}}
    catch(e){toast(e.message,true);}finally{busy=false;button.disabled=false;}
  }
  function init() {
    $("#detail-view").addEventListener("click",e=>{const b=e.target.closest("button");if(!b)return;if(b.hasAttribute("data-add-blocker"))open();if(b.dataset.removeBlocker)remove(Number(b.dataset.removeBlocker),b);if(b.dataset.createDependent)window.dispatchEvent(new CustomEvent("hey-boss-quick-issue-blocked-by",{detail:{number:Number(b.dataset.createDependent)}}));});
    $("#blocker-picker-search").oninput=options;
    $("#blocker-picker-results").onclick=e=>{const b=e.target.closest("[data-select-blocker]");if(b)add(Number(b.dataset.selectBlocker));};
    $("#blocker-picker-search").onkeydown=e=>{if(e.key==="ArrowDown"){e.preventDefault();$("[data-select-blocker]",$("#blocker-picker-results"))?.focus();}};
    $("#blocker-picker-results").onkeydown=e=>{if(!["ArrowUp","ArrowDown","Home","End"].includes(e.key))return;const buttons=$$("[data-select-blocker]:not(:disabled)",$("#blocker-picker-results"));if(!buttons.length)return;e.preventDefault();const n=buttons.indexOf(document.activeElement);buttons[e.key==="Home"?0:e.key==="End"?buttons.length-1:Math.max(0,Math.min(buttons.length-1,n+(e.key==="ArrowDown"?1:-1)))].focus();};
    for(const id of ["blocker-picker-close","blocker-picker-cancel"])$("#"+id).onclick=close;
    $("#blocker-picker-dialog").addEventListener("cancel",e=>{e.preventDefault();close();});
  }
  return {list,card,init,kind,matches,description};
})();
