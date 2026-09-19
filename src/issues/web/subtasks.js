"use strict";
// Loaded before app.js; helpers and model are used only after boot.
const IssueSubtasks = (() => {
  let picker = null, sequence = 0, busy = false, highlight = null;
  const href = number => routeHash({ ...model.route, view: "issues", issue: number });
  const progress = issue => issue.subtasks;
  function list(issue) {
    const p = progress(issue), parent = issue.parent;
    return `${parent ? `<a class="issue-parent-link" data-issue="${parent.number}" href="${esc(href(parent.number))}" title="${esc(parent.title)}${parent.deleted_at ? " (deleted)" : ""}">${icon("subtasks")}#${parent.number}</a>` : ""}${p?.total ? `<a class="issue-subtask-progress" data-issue="${issue.number}" href="${esc(href(issue.number))}" aria-label="${p.closed} of ${p.total} subtasks closed">${icon("subtasks")}<span>${p.closed}/${p.total}</span><span class="subtask-mini-track" aria-hidden="true"><i style="width:${Math.round(p.closed / p.total * 100)}%"></i></span></a>` : ""}`;
  }
  function parent(issue) {
    return issue.parent ? `<div class="parent-breadcrumb">${icon("subtasks")}<span>Subtask of</span><a data-issue="${issue.parent.number}" href="${esc(href(issue.parent.number))}">#${issue.parent.number} ${esc(issue.parent.title)}</a>${issue.parent.deleted_at ? '<span class="muted-text">Deleted</span>' : ""}</div>` : "";
  }
  function row(child, editable, deleted = false) {
    return `<li class="subtask-row issue-row${deleted ? " deleted-subtask" : ""}" data-issue-number="${child.number}">${editable && !deleted ? `<button type="button" class="issue-order-handle" data-move-issue="${child.number}" aria-keyshortcuts="ArrowUp ArrowDown" aria-label="Reorder subtask #${child.number}: ${esc(child.title)}" title="Drag to reorder. Use ↑ or ↓ when focused.">${icon("grip")}</button>` : '<span class="subtask-grip-space"></span>'}<span class="issue-state ${deleted ? "deleted" : child.state}">${icon(deleted ? "trash" : child.state === "closed" ? "closed" : "issue")}</span><div class="subtask-row-main"><a class="subtask-title" data-issue="${child.number}" href="${esc(href(child.number))}"><span class="subtask-number">#${child.number}</span> ${esc(child.title)}</a>${child.assignee ? listAssignee(child.assignee) : ""}${(child.labels || []).map(listLabel).join("")}${listPullRequests(child)}${list({...child,parent:null})}${!deleted && child.state === "closed" && child.closed_at ? `<span class="subtask-closed">Closed ${date(child.closed_at)}</span>` : ""}</div><button type="button" class="icon-button subtask-unlink" data-unlink-subtask="${child.number}" aria-label="Unlink subtask #${child.number}" title="Unlink; keep the issue">${icon("x")}</button></li>`;
  }
  function card(value) {
    const issue = value.issue, children = value.subtasks || [], p = issue.subtasks;
    const visible = children.filter(c => !c.deleted_at), deleted = children.filter(c => c.deleted_at), editable = !issue.deleted_at;
    if (!children.length) return "";
    return `<section class="subtasks-card" aria-labelledby="subtasks-heading"><div class="subtasks-heading"><h2 id="subtasks-heading" tabindex="-1">${icon("subtasks")}Subtasks${visible.length ? `<span>${p.closed}/${p.total}</span>` : ""}</h2>${editable ? '<div class="subtask-actions"><button type="button" class="button small" data-add-existing-subtask>Add existing</button></div>' : ""}</div>${visible.length ? `<div class="subtask-progress-line"><progress value="${p.closed}" max="${p.total}" aria-label="${p.closed} of ${p.total} subtasks closed"></progress><span>${p.closed} of ${p.total} closed</span></div><ul id="subtask-list" class="subtask-list">${visible.map(c => row(c,editable)).join("")}</ul>${p.open_descendants > p.total - p.closed ? `<p class="subtask-descendants">${p.open_descendants} open ${p.open_descendants===1?"issue":"issues"} across all levels</p>` : ""}` : ""}${deleted.length ? `<details class="deleted-subtasks"><summary>${deleted.length} deleted ${deleted.length === 1 ? "subtask" : "subtasks"}</summary><ul class="subtask-list">${deleted.map(c => row(c,false,true)).join("")}</ul></details>` : ""}</section>`;
  }
  function signature(value) {
    const parent = value.issue.parent;
    return JSON.stringify([parent && [parent.number,parent.title,parent.state,parent.deleted_at],value.issue.subtasks,(value.subtasks || []).map(c=>[c.number,c.title,c.state,c.assignee,c.deleted_at,c.version,c.closed_at,c.subtasks,c.labels,c.pull_requests])]);
  }
  function reveal(number) {
    highlight = {project:model.project.id,host:model.route.host,number};
  }
  function rendered() {
    if (!highlight || highlight.project !== model.project.id || highlight.host !== model.route.host) return;
    const row = $(`#subtask-list [data-issue-number="${highlight.number}"]`);
    if (!row) return;
    const link = $(".subtask-title",row);row.classList.add("issue-created");link.focus({preventScroll:true});row.scrollIntoView({block:"nearest"});
    const current = highlight;setTimeout(()=>{row.classList.remove("issue-created");if(highlight===current)highlight=null},4000);highlight=null;
  }
  function closePicker() {
    if (busy) return;
    ++sequence;$("#subtask-picker-dialog").close();
    picker?.returnFocus?.isConnected && picker.returnFocus.focus({preventScroll:true});picker=null;
  }
  function options() {
    const target=$("#subtask-picker-results"), query=$("#subtask-picker-search").value.trim().toLowerCase();
    const matches=(picker?.issues || []).filter(i => !query || `${i.number} ${i.title}`.toLowerCase().includes(query));
    target.innerHTML=matches.length ? matches.map(i=>{
      const unavailable=i.number===picker.parent.number || picker.ancestors.has(i.number) || !!i.parent;
      const reason=i.number===picker.parent.number ? "Current issue" : picker.ancestors.has(i.number) ? "Parent issue" : i.parent ? `Subtask of #${i.parent.number}` : i.state==="closed" ? "Closed" : "Open";
      return `<button type="button" class="subtask-option" data-existing-subtask="${i.number}" ${unavailable || busy ? "disabled" : ""}><span class="issue-state ${i.state}">${icon(i.state==="closed"?"closed":"issue")}</span><span><strong><small>#${i.number}</small> ${esc(i.title)}</strong><span>${reason}</span></span>${unavailable ? "" : icon("plus")}</button>`;
    }).join("") : '<p class="picker-empty">No matching issues.</p>';
    target.classList.toggle("large-picker",matches.length>300);$("#subtask-picker-count").textContent=`${matches.length} ${matches.length===1?"issue":"issues"}`;
  }
  async function openPicker(returnFocus = $("[data-add-existing-subtask]")) {
    const local=++sequence;picker={project:model.project.id,host:model.route.host,parent:model.detail.issue,returnFocus,issues:[],ancestors:new Set()};
    $("#subtask-picker-search").value="";$("#subtask-picker-error").hidden=true;$("#subtask-picker-count").textContent="";$("#subtask-picker-results").innerHTML='<div class="loading-state"><span class="spinner"></span></div>';
    $("#subtask-picker-dialog").showModal();$("#subtask-picker-search").focus();
    try {
      const value=await api({action:"list",state:"all",mine:false,unassigned:false,labels:[],search:null,limit:100,offset:0,all:true},picker.project,null,picker.host);
      if(local!==sequence || !picker)return;
      picker.issues=value.issues;let parent=picker.parent;
      const seen=new Set();while(parent?.parent && !seen.has(parent.number)){seen.add(parent.number);picker.ancestors.add(parent.parent.number);parent=picker.issues.find(i=>i.number===parent.parent.number);}
      options();
    } catch(error) {if(local!==sequence)return;$("#subtask-picker-error").textContent=error.message;$("#subtask-picker-error").hidden=false;$("#subtask-picker-results").innerHTML='<button type="button" class="button" id="subtask-picker-retry">Retry</button>';$("#subtask-picker-retry").onclick=()=>{closePicker();openPicker()};}
  }
  async function link(number) {
    if(busy || !picker)return;busy=true;const context=picker;
    $("#subtask-picker-search").disabled=true;options();$("#subtask-picker-error").hidden=true;
    try {
      const child=context.issues.find(i=>i.number===number);saveComment();
      await mutate({action:"add_subtask",number:context.parent.number,child:number,if_version:context.parent.version,if_child_version:child.version},context.project,context.host);
      busy=false;closePicker();reveal(number);await renderRoute();toast("Subtask added");
    } catch(error) {
      $("#subtask-picker-error").textContent=error.message;$("#subtask-picker-error").hidden=false;
      if(error.code==="conflict"){
        try {
          const [current, choices]=await Promise.all([
            api({action:"view",number:context.parent.number},context.project,null,context.host),
            api({action:"list",state:"all",mine:false,unassigned:false,labels:[],search:null,limit:100,offset:0,all:true},context.project,null,context.host)
          ]);
          if(picker===context){context.parent=current.issue;context.issues=choices.issues;}
        }catch{}
      }
    } finally {busy=false;$("#subtask-picker-search").disabled=false;if(picker===context)options();}
  }
  async function unlink(number,button) {
    if(busy)return;busy=true;button.disabled=true;saveComment();
    const project=model.project.id,parent=model.detail.issue,child=(model.detail.subtasks||[]).find(c=>c.number===number),route=model.sequence;
    try {
      await mutate({action:"remove_subtask",number:parent.number,child:number,if_version:parent.version,if_child_version:child.version},project);
      if(route!==model.sequence)return;await renderRoute();($("#subtasks-heading") || $("[data-create-subtask]"))?.focus({preventScroll:true});toast(`Subtask unlinked. Issue #${number} is preserved.`);
    } catch(error){toast(error.message,true);}finally{busy=false;button.disabled=false;}
  }
  function init() {
    $("#detail-view").addEventListener("click",event=>{
      const button=event.target.closest("button");if(!button)return;
      if(busy)return;
      if(button.hasAttribute("data-create-subtask"))openEditor(null,{parent:model.detail.issue});
      if(button.hasAttribute("data-add-existing-subtask"))openPicker(button);
      if(button.dataset.unlinkSubtask)unlink(Number(button.dataset.unlinkSubtask),button);
    });
    $("#subtask-picker-search").oninput=options;
    $("#subtask-picker-results").onclick=event=>{const button=event.target.closest("[data-existing-subtask]");if(button)link(Number(button.dataset.existingSubtask));};
    $("#subtask-picker-results").onkeydown=event=>{
      if(!["ArrowUp","ArrowDown","Home","End"].includes(event.key))return;
      const buttons=$$("[data-existing-subtask]:not(:disabled)",$("#subtask-picker-results"));if(!buttons.length)return;event.preventDefault();
      let index=buttons.indexOf(document.activeElement);index=event.key==="Home"?0:event.key==="End"?buttons.length-1:Math.max(0,Math.min(buttons.length-1,index+(event.key==="ArrowDown"?1:-1)));buttons[index].focus();
    };
    $("#subtask-picker-search").onkeydown=event=>{if(event.key==="ArrowDown"){const first=$("[data-existing-subtask]:not(:disabled)",$("#subtask-picker-results"));if(first){event.preventDefault();first.focus();}}};
    for(const id of ["subtask-picker-close","subtask-picker-cancel"])$("#"+id).onclick=closePicker;
    $("#subtask-picker-dialog").addEventListener("cancel",event=>{event.preventDefault();closePicker();});
  }
  return {list,parent,card,signature,reveal,rendered,init};
})();
