"use strict";
const HeyBossArtifacts = (() => {
  const $ = (s, root=document) => root.querySelector(s);
  const esc = s => String(s??"").replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[c]);
  const failure=(message,uncertain=false)=>Object.assign(Error(message),{uncertain});
  const reads = new Set(["list","view","links","preview"]);
  const url = (project,id,extra={}) => `/artifacts#${new URLSearchParams({project,...(id?{artifact:id}:{}),...extra})}`;
  let mobile = false;
  async function rpc(context,operation,reading,requestID) {
    const payload = {project:context.project,operation,...(context.host?{host:context.host}:{}),request_id:reading?null:(requestID||crypto.randomUUID())};
    let response;try { response = await fetch(mobile?"/api/artifact-requests":"/api/action",{method:"POST",headers:{"Content-Type":"application/json",...(mobile?{}:{"X-Hey-Boss-CSRF":context.csrf})},body:JSON.stringify(payload),signal:AbortSignal.timeout(20000)}); } catch(e) {throw failure("Connection interrupted. Retry this pending save with the same content after reconnecting.",true);}
    let value;try {value = await response.json();}catch(e){throw failure("Delivery could not be confirmed. Retry the pending save after reconnecting.",true);}
    if (!response.ok) throw failure(value.error?.message||value.error||"Could not connect; your draft is preserved.",response.status>=500);
    if (mobile) {
      const request=value.request;
      for(let i=0;i<90;i++) {
        await new Promise(resolve=>setTimeout(resolve,1000));
        try {response=await fetch(`/api/artifact-requests/${request.id}`,{signal:AbortSignal.timeout(20000)});value=await response.json();}catch(e){throw failure("Delivery is pending. Retry the same save after reconnecting.",true);}
        if(!response.ok) throw failure(value.error||"Reconnect to load this request",true);
        if(value.request.status!=="pending") {value=value.request.result;break;}
        if(i===89) throw failure("Supervisor is offline or still processing. Your pending save is preserved; retry after reconnecting.",true);
      }
    }
    if (!value.ok) throw Error(value.error?.message||"Could not save; your draft is preserved.");
    return value;
  }
  const api=(context,operation,requestID)=>rpc(context,{action:"artifact",operation},reads.has(operation.command),requestID);
  function mount(root,context) {
    if(!root)return;
    const render = artifacts => {
      root.classList.add("artifact-relationship");
      root.innerHTML=`<h2 class="side-heading">Artifacts</h2><ul>${artifacts.map(a=>`<li><a href="${esc(url(context.project,a.id,context.host?{host:context.host}:{}))}">${esc(a.title)}${a.archived?" (archived)":""}</a><button type="button" data-unlink="${esc(a.id)}" aria-label="Unlink ${esc(a.title)}">Unlink</button></li>`).join("")}</ul><a class="button" href="${esc(url(context.project,null,{new:"1",...(context.issue?{issue:context.issue}:{node:context.node}),...(context.host?{host:context.host}:{})}))}">Create artifact</a><button type="button" class="button" data-attach>Attach existing</button><div data-attach-form></div><p role="alert" hidden></p>`;
      const error=e=>{const p=$("[role=alert]",root);p.hidden=false;p.textContent=e.message;};
      const target=context.issue?{issue:context.issue}:{node:context.node};
      root.querySelectorAll("[data-unlink]").forEach(b=>b.onclick=async()=>{b.disabled=true;try{await api(context,{command:"unlink",id:b.dataset.unlink,...target});render(artifacts.filter(a=>a.id!==b.dataset.unlink));}catch(e){b.disabled=false;error(e);}});
      $("[data-attach]",root).onclick=async()=>{
        try {
          const form=$("[data-attach-form]",root);
          form.innerHTML='<form><label>Find artifact <input type="search" aria-label="Find artifact to attach"></label><select aria-label="Artifact to attach"></select><button class="button" type="submit">Attach</button></form>';
          const load=async()=>{const v=await api(context,{command:"list",query:$("input",form).value,archived:false});if(!form.isConnected)return;$("select",form).innerHTML=v.artifacts.map(a=>`<option value="${esc(a.id)}">${esc(a.title)}</option>`).join("");};
          let timer;$("input",form).oninput=()=>{clearTimeout(timer);timer=setTimeout(()=>load().catch(error),250);};
          $("form",form).onsubmit=async e=>{e.preventDefault();const id=$("select",form).value;if(!id)return;const button=$("button",form);button.disabled=true;try{await api(context,{command:"link",id,...target});const v=await api(context,{command:"links",...target});render(v.artifacts);}catch(err){button.disabled=false;error(err);}};
          await load();$("input",form).focus();
        }catch(e){error(e);}
      };
    };
    render(context.artifacts||[]);
  }

  async function start() {
    if(!$("#artifact-main"))return;
    const route=()=>new URLSearchParams(location.hash.slice(1));
    const status=s=>{$("#artifact-status").textContent=s;};
    const error=e=>{$("#artifact-error").hidden=false;$("#artifact-error").textContent=e.message;status("Save failed · draft preserved");};
    let boot,context,project,doc=null,editing=false,generation=0,offset=0,rows=[],anchor=null;
    try {mobile=document.documentElement.dataset.artifactMobile==="true";const r=await fetch(mobile?"/api/artifact-bootstrap":"/api/bootstrap");boot=await r.json();if(!r.ok)throw Error(boot.error?.message||boot.error||"Pair this device to continue");}catch(e){error(e);return;}
    const picker=new HeyBossUI.ProjectPicker({onSelect:id=>{location.hash=new URLSearchParams({project:id});}});
    const draftKey=id=>`hey-boss-artifact-draft:${context.host||"local"}:${context.project}:${id||"new"}`;
    const drafts={get:key=>{try{return JSON.parse(localStorage.getItem(key));}catch{return null;}},set:(key,v)=>{try{localStorage.setItem(key,JSON.stringify(v));return true;}catch{return false;}},remove:key=>{try{localStorage.removeItem(key);}catch{}}};
    function keepDraft() {
      if(!editing)return;
      const key=draftKey(doc?.id),value={title:$("#artifact-title").value,body:$("#artifact-body").value,version:Number($("#artifact-editor").dataset.version),requestID:$("#artifact-editor").dataset.requestId,pending:$("#artifact-editor").dataset.pending?JSON.parse($("#artifact-editor").dataset.pending):null};
      const saved=drafts.set(key,value);status(saved?"Draft saved on this browser · publish with Save":"Draft is only in this tab · browser storage unavailable");
    }
    function editor() {
      editing=true;
      const draft=drafts.get(draftKey(doc?.id));
      $("#artifact-library").hidden=true;$("#artifact-document").hidden=false;
      $("#artifact-document").innerHTML=`<form id="artifact-editor" class="artifact-editor" data-version="${draft?.version||doc?.version||1}" data-request-id="${esc(draft?.requestID||crypto.randomUUID())}"><h2>${doc?"Edit artifact":"New artifact"}</h2><label>Title<input id="artifact-title" required maxlength="512" value="${esc(draft?.title??doc?.title??"")}"></label><label>Markdown<textarea id="artifact-body">${esc(draft?.body??doc?.body??"")}</textarea></label><div class="artifact-actions"><button class="button primary" type="submit">Save</button><button class="button" type="button" id="artifact-preview">Preview</button><label class="button">Import Markdown<input id="artifact-import" type="file" accept=".md,.markdown,text/markdown,text/plain"></label><button class="button" type="button" id="artifact-cancel">Back</button></div><div class="markdown artifact-reading" id="artifact-edit-preview" hidden></div><div id="artifact-conflict" class="artifact-conflict"></div></form>`;
      let saving=false,pending=draft?.pending||null;
      if(pending){$("#artifact-editor").dataset.pending=JSON.stringify(pending);$("#artifact-title").disabled=true;$("#artifact-body").disabled=true;}
      for(const input of [$("#artifact-title"),$("#artifact-body")]) input.oninput=()=>{if(!saving&&!pending){$("#artifact-editor").dataset.requestId=crypto.randomUUID();pending=null;}keepDraft();};
      $("#artifact-cancel").onclick=()=>{keepDraft();editing=false;doc?reading():location.hash=new URLSearchParams({project:context.project});};
      $("#artifact-import").onchange=async e=>{const file=e.target.files[0];if(!file)return;if(file.size>1048576){error(Error("Markdown must be at most 1 MiB"));return;}$("#artifact-body").value=await file.text();$("#artifact-body").dispatchEvent(new Event("input"));};
      $("#artifact-preview").onclick=async()=>{try{const panel=$("#artifact-edit-preview");const v=await api(context,{command:"preview",body:$("#artifact-body").value});panel.hidden=false;panel.innerHTML=v.html;}catch(e){error(e);}};
      $("#artifact-editor").onsubmit=async e=>{
        e.preventDefault();if(saving)return;keepDraft();const form=e.currentTarget,key=draftKey(doc?.id),title=$("#artifact-title").value,body=$("#artifact-body").value;
        const target=route().get("issue")?{issue:Number(route().get("issue"))}:route().get("node")?{node:route().get("node")}:{};
        pending ||= doc?{command:"edit",id:doc.id,title,body,if_version:Number(form.dataset.version)}:{command:"create",title,body,...target};
        form.dataset.pending=JSON.stringify(pending);keepDraft();saving=true;form.querySelectorAll("input,textarea,button").forEach(el=>el.disabled=true);status("Saving…");$("#artifact-error").hidden=true;
        try {const v=await api(context,pending,form.dataset.requestId);drafts.remove(key);if(!form.isConnected)return;doc=v.artifact;doc.result=v;editing=false;status("Saved");location.hash=new URLSearchParams({project:context.project,artifact:doc.id,...(context.host?{host:context.host}:{})});reading();}
        catch(err){if(!form.isConnected)return;error(err);if(!err.uncertain){pending=null;form.dataset.pending="";form.dataset.requestId=crypto.randomUUID();keepDraft();}else{status("Save pending · retry with the same content to avoid duplicate documents");}if(doc&&!err.uncertain){$("#artifact-conflict").innerHTML='<button class="button" id="artifact-load-latest" type="button">Load latest for comparison</button>';$("#artifact-load-latest").onclick=async()=>{try{const latest=await api(context,{command:"view",id:doc.id});if(!form.isConnected)return;const panel=$("#artifact-conflict");panel.innerHTML=`<h3>Latest saved revision ${latest.artifact.version}</h3><strong>${esc(latest.artifact.title)}</strong><pre>${esc(latest.artifact.body)}</pre><p>Merge the saved changes into your draft above, then use this revision to save.</p><button type="button" class="button" id="artifact-use-revision">Use revision ${latest.artifact.version} for merged draft</button>`;$("#artifact-use-revision").onclick=()=>{form.dataset.version=latest.artifact.version;form.dataset.requestId=crypto.randomUUID();pending=null;keepDraft();panel.innerHTML="<p>Latest revision selected. Review your merged draft and Save.</p>";};}catch(e){error(e);}};}}
        finally {saving=false;if(form.isConnected){form.querySelectorAll("input,textarea,button").forEach(el=>el.disabled=false);if(pending){$("#artifact-title").disabled=true;$("#artifact-body").disabled=true;$("#artifact-import").disabled=true;}}}
      };
      status(pending?"Recovered pending save · retry to confirm delivery":draft?"Recovered browser draft":"Markdown editor");$("#artifact-title").focus();
    }
    function reading() {
      editing=false;anchor=null;const v=doc.result;
      $("#artifact-library").hidden=true;$("#artifact-document").hidden=false;
      const comment=c=>`<div class="markdown">${c.body_html}</div><p class="artifact-muted">${esc(c.author)} · ${new Date(c.created_at).toLocaleString()}</p>`;
      const threads=v.comments.filter(c=>!c.parent).map(c=>{
        const body=`${c.quote?`<blockquote>${esc(c.quote)}</blockquote>${c.outdated?'<p class="artifact-muted">Outdated selection · discussion preserved</p>':""}`:""}${comment(c)}${v.comments.filter(r=>r.parent===c.id).map(r=>`<div class="reply">${comment(r)}</div>`).join("")}<form data-reply="${c.id}"><label>Reply<textarea aria-label="Reply to comment"></textarea></label><button class="button" type="submit">Reply</button></form><button class="button" type="button" data-resolve="${c.id}" data-resolved="${!c.resolved}">${c.resolved?"Reopen thread":"Resolve"}</button>`;
        return c.resolved?`<details class="artifact-thread resolved"><summary>Resolved · ${esc(c.body.slice(0,70))}</summary>${body}</details>`:`<section class="artifact-thread">${body}</section>`;
      }).join("");
      $("#artifact-document").innerHTML=`<a class="back-link" href="${esc(url(context.project,null,context.host?{host:context.host}:{}))}">← All artifacts</a><h1>${esc(doc.title)}${doc.archived?' <span class="artifact-muted">Archived</span>':""}</h1><p class="artifact-muted">Revision ${doc.version} · Updated ${new Date(doc.updated_at).toLocaleString()}</p><div class="artifact-actions"><button class="button" id="artifact-edit">Edit / rename</button><button class="button" id="artifact-export">Export Markdown</button><button class="button" id="artifact-archive">${doc.archived?"Restore":"Archive"}</button></div><div class="artifact-layout"><div><article id="artifact-reading" class="markdown artifact-reading">${doc.body_html||'<p class="artifact-muted">Empty document</p>'}</article><section><h2>Linked from</h2><ul class="artifact-links">${v.backlinks.map(l=>{const href=mobile?`/project-resource#${new URLSearchParams({project:context.project,[l.kind==="issue"?"issue":"node"]:l.target})}`:l.kind==="issue"?`/#${new URLSearchParams({project:context.project,issue:l.target,...(context.host?{host:context.host}:{})})}`:`/mm#${new URLSearchParams({project:context.project,node:l.target})}`;return `<li>${l.title?`<a href="${esc(href)}">${esc(l.title)}${l.kind==="issue"?` · #${esc(l.target)}`:""}</a>`:`<span class="artifact-muted">Removed ${esc(l.kind)} · ${esc(l.target)}</span>`}</li>`;}).join("")||'<li class="artifact-muted">No references yet</li>'}</ul></section></div><aside aria-label="Document comments"><h2>Comments</h2><p class="artifact-muted">Select document text to anchor a comment.</p><div id="artifact-threads">${threads||'<p class="artifact-muted">Start a conversation</p>'}</div><form id="artifact-comment-form"><blockquote id="artifact-quote" class="artifact-quote" hidden></blockquote><button type="button" id="artifact-clear-quote" hidden>Comment on whole document</button><label for="artifact-comment">Add a comment</label><textarea id="artifact-comment" required></textarea><button class="button primary" type="submit">Comment</button></form></aside></div>`;
      document.title=`${doc.title} · Artifacts · Hey Boss`;
      $("#artifact-edit").onclick=editor;
      $("#artifact-export").onclick=()=>{const blob=new Blob([doc.body],{type:"text/markdown;charset=utf-8"}),a=document.createElement("a");a.href=URL.createObjectURL(blob);a.download=(doc.title.replace(/[^\p{L}\p{N}_ -]/gu,"_")||"artifact")+".md";a.click();setTimeout(()=>URL.revokeObjectURL(a.href),1000);};
      const mutate=async(op,button)=>{button.disabled=true;try{const r=await api(context,op);if(!button.isConnected)return;doc=r.artifact;doc.result=r;reading();status("Saved");}catch(e){button.disabled=false;error(e);}};
      $("#artifact-archive").onclick=e=>mutate({command:"archive",id:doc.id,archived:!doc.archived,if_version:doc.version},e.currentTarget);
      $("#artifact-document").querySelectorAll("[data-resolve]").forEach(b=>b.onclick=()=>mutate({command:"resolve",id:doc.id,comment_id:Number(b.dataset.resolve),resolved:b.dataset.resolved==="true"},b));
      $("#artifact-document").querySelectorAll("[data-reply]").forEach(f=>f.onsubmit=e=>{e.preventDefault();const body=$("textarea",f).value;if(body.trim())mutate({command:"comment",id:doc.id,parent:Number(f.dataset.reply),body},$("button",f));});
      const commentKey=`${draftKey(doc.id)}:comment`;
      let pendingComment=drafts.get(commentKey)?.pending||null;
      $("#artifact-comment").value=drafts.get(commentKey)?.body||"";$("#artifact-comment").readOnly=!!pendingComment;
      $("#artifact-comment").oninput=()=>drafts.set(commentKey,{body:$("#artifact-comment").value,anchor,pending:pendingComment});
      const restored=drafts.get(commentKey)?.anchor;
      const showAnchor=()=>{$("#artifact-quote").hidden=!anchor;$("#artifact-quote").textContent=anchor?.quote||"";$("#artifact-clear-quote").hidden=!anchor;};
      if(restored){anchor=restored;showAnchor();}
      $("#artifact-clear-quote").onclick=()=>{if(pendingComment)return;anchor=null;showAnchor();$("#artifact-comment").dispatchEvent(new Event("input"));};
      const select=()=>{const selection=getSelection(),reading=$("#artifact-reading");if(!selection.rangeCount||selection.isCollapsed)return;const range=selection.getRangeAt(0);if(!reading.contains(range.commonAncestorContainer))return;const quote=selection.toString();if(!quote.trim()||pendingComment)return;if(new TextEncoder().encode(quote).length>8192){error(Error("Select at most 8 KiB of text for a comment"));return;}const before=range.cloneRange();before.selectNodeContents(reading);before.setEnd(range.startContainer,range.startOffset);const after=range.cloneRange();after.selectNodeContents(reading);after.setStart(range.endContainer,range.endOffset);anchor={quote,prefix:before.toString().slice(-80),suffix:after.toString().slice(0,80)};showAnchor();$("#artifact-comment").dispatchEvent(new Event("input"));};
      $("#artifact-reading").onpointerup=select;$("#artifact-reading").onkeyup=select;
      $("#artifact-comment-form").onsubmit=async e=>{e.preventDefault();const b=$("button[type=submit]",e.currentTarget);b.disabled=true;pendingComment ||= {operation:{command:"comment",id:doc.id,body:$("#artifact-comment").value,...(anchor||{})},requestID:crypto.randomUUID()};$("#artifact-comment").readOnly=true;$("#artifact-comment").dispatchEvent(new Event("input"));try{const r=await api(context,pendingComment.operation,pendingComment.requestID);drafts.remove(commentKey);if(!b.isConnected)return;doc=r.artifact;doc.result=r;reading();status("Comment saved");}catch(e){if(!b.isConnected)return;b.disabled=false;if(!e.uncertain){pendingComment=null;$("#artifact-comment").readOnly=false;$("#artifact-comment").dispatchEvent(new Event("input"));}error(e);}};
    }
    async function library(append=false) {
      const seq=++generation;status("Loading…");
      try {const value=await api(context,{command:"list",query:$("#artifact-search").value,archived:$("#artifact-archived").checked,offset:append?offset:0});if(seq!==generation)return;rows=append?[...rows,...value.artifacts]:value.artifacts;offset=rows.length;
        $("#artifact-list").innerHTML=rows.map(a=>`<a class="artifact-row" href="${esc(url(context.project,a.id,context.host?{host:context.host}:{}))}"><strong>${esc(a.title)}</strong><span>${a.archived?"Archived · ":""}Updated ${new Date(a.updated_at).toLocaleString()} · revision ${a.version}</span></a>`).join("")||'<p class="artifact-muted">No artifacts found in this project</p>';
        $("#artifact-more").hidden=!value.more;status("Recently updated documents");
      }catch(e){if(seq===generation)error(e);}
    }
    async function navigate() {
      if(editing)keepDraft();editing=false;generation++;$("#artifact-error").hidden=true;
      const params=route(),id=HeyBossUI.projectId(boot.projects,params.get("project"));project=boot.projects.find(p=>p.id===id)||boot.projects[0];
      if(!project){error(Error("No registered projects. Reconnect the supervisor to load project data."));return;}
      context={project:project.id,csrf:boot.csrf,host:params.get("host")};picker.update(boot.projects,project);doc=null;
      $("#nav-artifacts").setAttribute("aria-current","page");
      if(mobile){$("#quick-issue-open").hidden=true;$("#nav-inbox").href="/";$("#nav-issues").href="/#issues";$("#nav-workers").hidden=true;$("#nav-mindmaps").hidden=true;}
      const idDoc=params.get("artifact");
      if(idDoc){const seq=++generation;status("Loading…");try{const v=await api(context,{command:"view",id:idDoc});if(seq!==generation)return;doc=v.artifact;doc.result=v;reading();status("Saved");}catch(e){error(e);}}
      else if(params.get("new")==="1")editor();
      else {$("#artifact-library").hidden=false;$("#artifact-document").hidden=true;await library();}
    }
    $("#artifact-new").onclick=()=>{location.hash=new URLSearchParams({project:context.project,new:"1",...(context.host?{host:context.host}:{})});};
    let timer;$("#artifact-search").oninput=()=>{clearTimeout(timer);timer=setTimeout(()=>library(),250);};
    $("#artifact-archived").onchange=()=>library();$("#artifact-more").onclick=()=>library(true);
    window.addEventListener("hashchange",navigate);window.addEventListener("beforeunload",()=>keepDraft());
    await navigate();
  }
  async function startResource() {
    if(!$("#resource-main"))return;
    mobile=true;
    const params=new URLSearchParams(location.hash.slice(1));
    try {
      const response=await fetch("/api/artifact-bootstrap"),boot=await response.json();
      if(!response.ok)throw Error(boot.error||"Pair this device to continue");
      const project=boot.projects.find(p=>p.id===params.get("project"));if(!project)throw Error("Project is unavailable");
      new HeyBossUI.ProjectPicker({onSelect:id=>location.href=url(id)}).update(boot.projects,project);
      $("#quick-issue-open").hidden=true;$("#nav-inbox").href="/";$("#nav-issues").href="/#issues";$("#nav-workers").hidden=true;$("#nav-mindmaps").hidden=true;
      const context={project:project.id};
      const issue=params.get("issue"),node=params.get("node");
      const result=await rpc(context,issue?{action:"view",number:Number(issue)}:{action:"mindmap",operation:{command:"view",node,body_mode:"full"}},true);
      const resource=issue?result.issue:result.nodes.find(n=>n.id===node)||result.nodes[0];
      if(!resource)throw Error("Resource was removed");
      if(!resource.body_html){resource.body_html=(await api(context,{command:"preview",body:resource.body||""})).html;}
      $("#resource-content").innerHTML=`<h1>${esc(resource.display_label||resource.title)}</h1><p class="artifact-muted">${issue?`Issue #${esc(issue)} · ${esc(resource.state)}`:"Mindmap topic"}</p><article class="markdown artifact-reading">${resource.body_html||esc(resource.body)}</article>`;
      const links=await api(context,issue?{command:"links",issue:Number(issue)}:{command:"links",node:resource.id});
      mount($("#resource-artifacts"),{...context,...(issue?{issue:Number(issue)}:{node:resource.id}),artifacts:links.artifacts});
      $("#resource-status").textContent="";
    }catch(e){$("#resource-status").textContent=e.message;}
  }
  startResource();
  start();
  return {mount,url};
})();
