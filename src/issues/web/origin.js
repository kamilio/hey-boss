"use strict";
const HeyBossOrigin = (() => {
  const esc = value => String(value ?? "").replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[c]);
  function creator(issue, name) {
    const model = issue.origin?.model;
    return issue.origin?.kind === 'codex' && typeof model === 'string' && model.trim() && model.length <= 256 && !/[\x00-\x1f\x7f]/.test(model)
      ? `Codex · ${model.trim()}` : name(issue.created_by);
  }
  function conversation(origin, project) {
    if (!origin?.session_id || origin.kind !== "codex") return null;
    const params = new URLSearchParams({project:origin.run?.project_id || project,host:origin.host,run:origin.run?.id || "session:"+origin.session_id});
    if (Number.isSafeInteger(origin.invocation?.offset)) params.set("at",origin.invocation.offset);
    return "/agents/session#"+params;
  }
  function card(origin, project, name = id => id === "human:boss" ? "Boss" : id?.startsWith("codex:") ? "Codex" : id || "Unknown") {
    if (!origin) return '<section class="origin-card" aria-label="Origin"><h2>Origin</h2><p class="origin-note">Creation context was not recorded.</p></section>';
    const href=conversation(origin,project), invocation=href&&origin.invocation;
    const issue=origin.run?.number;
    const mobile=document.documentElement.dataset.artifactMobile==='true';
    const issueURL=(mobile?'/project-resource#':'/#')+new URLSearchParams({project:origin.run?.project_id||project,issue});
    return `<section class="origin-card" aria-label="Origin"><h2>Origin</h2><p class="origin-author">Created by <strong>${esc(creator({created_by:origin.actor_id,origin},name))}</strong></p><p class="origin-device">${esc(origin.host)}</p>${issue?`<a class="origin-task" href="${esc(issueURL)}">While working on #${esc(issue)}${origin.run.title?` · ${esc(origin.run.title)}`:""}</a>`:""}${href?`<a class="origin-conversation" href="${esc(href)}">${invocation?"View creating invocation":"View creator conversation"}<span aria-hidden="true">↗</span></a>`:origin.session_id?'<p class="origin-note">This session’s conversation viewer is unavailable.</p>':""}${origin.session_id?`<details class="origin-context"><summary>Session details</summary><dl><dt>Session</dt><dd>${esc(origin.session_id)}</dd><dt>Checkout</dt><dd>${esc(origin.cwd)}</dd>${origin.invocation?.call_id?`<dt>Invocation</dt><dd>${esc(origin.invocation.call_id)}</dd>`:""}</dl></details>`:""}</section>`;
  }
  return {card,conversation,creator};
})();
if(typeof module!=="undefined")module.exports=HeyBossOrigin;
