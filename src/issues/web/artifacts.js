"use strict";
const HeyBossArtifacts = (() => {
  const $ = (s, root=document) => root.querySelector(s);
  const esc = s => String(s??"").replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[c]);
  const failure=(message,uncertain=false)=>Object.assign(Error(message),{uncertain});
  const reads = new Set(["list","view","links","preview"]);
  const url = (project,id,extra={}) => `/artifacts#${new URLSearchParams({project,...(id?{artifact:id}:{}),...extra})}`;
  const resourceURL=(context,kind,target)=>{
    const params=new URLSearchParams({project:context.project,[kind==="issue"?"issue":"node"]:target,...(!mobile&&context.host?{host:context.host}:{})});
    return `${mobile?"/project-resource":kind==="issue"?"/":"/mm"}#${params}`;
  };
  let mobile = false;
  const author=value=>value==="human:boss"?"Boss":value.startsWith("human:")?value.slice(6):value.startsWith("codex:")?"Codex":value.startsWith("claude:")?"Claude":value;
  async function rpc(context,operation,reading,requestID) {
    const payload = {project:context.project,operation,...(context.host?{host:context.host}:{}),request_id:reading?null:(requestID||HeyBossUI.requestId())};
    let response;try { response = await fetch(mobile?"/api/artifact-requests":"/api/action",{method:"POST",headers:{"Content-Type":"application/json",...(mobile?{}:{"X-Hey-Boss-CSRF":context.csrf})},body:JSON.stringify(payload),signal:AbortSignal.timeout(20000)}); } catch(e) {throw failure("Connection interrupted. "+(reading?"Reconnect and try again.":"Retry this pending save with the same content after reconnecting."),!reading);}
    let value;try {value = await response.json();}catch(e){throw failure("Delivery could not be confirmed. "+(reading?"Reconnect and try again.":"Retry the pending save after reconnecting."),!reading);}
    if (!response.ok) throw Object.assign(failure(value.error?.message||value.error||"Could not connect; your draft is preserved.",response.status>=500),{code:value.error?.code});
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
    if (!value.ok) throw Object.assign(failure(value.error?.message||"Could not save; your draft is preserved.",operation.action==="artifact"&&operation.operation.command==="delete"&&value.error?.code==="io_error"),{code:value.error?.code});
    return value;
  }
  const api=(context,operation,requestID)=>rpc(context,{action:"artifact",operation},reads.has(operation.command),requestID);
  function anchorText(range) {
    if(!range.commonAncestorContainer.querySelector?.(".artifact-diagram"))return range.toString();
    const fragment=range.cloneContents();
    // The server anchors against rendered Markdown. Keep original code text,
    // excluding generated SVG labels and diagram controls from that context.
    for(const diagram of fragment.querySelectorAll(".artifact-diagram")){
      const source=diagram.querySelector("pre");
      if(source)diagram.replaceWith(source);else diagram.remove();
    }
    return fragment.textContent;
  }
  // Only split at complete top-level elements. Quoted attributes may contain >;
  // lists, code, tables and inline markup retain their original HTML structure.
  function* markdownBlocks(html) {
    const tags=/<\/?([a-z][a-z0-9-]*)\b(?:"[^"]*"|'[^']*'|[^'">])*>|<!--[\s\S]*?-->/gi;
    const voids=new Set(["area","base","br","col","embed","hr","img","input","link","meta","param","source","track","wbr"]);
    let depth=0,start=0,match;
    while((match=tags.exec(html))) {
      if(match[1]) {
        if(match[0].startsWith("</"))depth--;
        else if(!voids.has(match[1].toLowerCase())&&!match[0].endsWith("/>"))depth++;
      }
      if(depth===0){yield html.slice(start,tags.lastIndex);start=tags.lastIndex;}
    }
    if(start<html.length)yield html.slice(start);
  }
  function externalLinks(root) {
    for(const link of root.querySelectorAll("a[href]")) {
      let destination;
      try{destination=new URL(link.getAttribute("href"),document.baseURI);}catch{continue;}
      if(!["http:","https:"].includes(destination.protocol)||destination.origin===location.origin)continue;
      link.target="_blank";
      link.relList.add("noopener","noreferrer");
    }
  }
  async function appendMarkdown(root,html) {
    const template=document.createElement("template");
    let buffer="";
    const flush=async()=>{
      if(!buffer)return true;
      await new Promise(requestAnimationFrame);
      if(!root.isConnected)return false;
      template.innerHTML=buffer;externalLinks(template.content);root.appendChild(template.content);buffer="";
      return true;
    };
    for(const block of markdownBlocks(html)) {
      // Open large structural containers once, then append complete siblings.
      // A 28,000-item list stays one list; tables keep their head/body sections.
      const container=block.length>=32000&&/^(\s*<(ul|ol|table|thead|tbody|blockquote|div)\b(?:"[^"]*"|'[^']*'|[^'">])*>)([\s\S]*)(<\/\2>\s*)$/i.exec(block);
      if(container){
        if(!await flush())return false;
        await new Promise(requestAnimationFrame);
        if(!root.isConnected)return false;
        template.innerHTML=container[1]+container[4];
        const parent=template.content.querySelector(container[2]);root.appendChild(template.content);
        if(!await appendMarkdown(parent,container[3]))return false;
      }else{
        buffer+=block;
        if(buffer.length>=12000&&!await flush())return false;
      }
    }
    return flush();
  }
  async function renderMarkdown(root,html,context=null,files=[]) {
    if(files.length){
      const template=document.createElement("template");template.innerHTML=html;
      for(const img of template.content.querySelectorAll("img[src]")){
        const decode=value=>{try{return decodeURI(value);}catch{return value;}};
        const file=files.find(file=>decode(file.destination)===decode(img.getAttribute("src")));
        if(file){img.dataset.importSrc=file.destination;img.removeAttribute("src");}
      }
      html=template.innerHTML;
    }
    root.classList.toggle("artifact-large",html.length>=32000);
    if(html.length<32000){root.innerHTML=html;externalLinks(root);}
    else{
      root.setAttribute("aria-busy","true");root.replaceChildren();
      if(!await appendMarkdown(root,html))return;
      root.removeAttribute("aria-busy");
    }
    if(context)HeyBossAttachments.hydrate(root,context,files);
    if(!root.querySelector("pre > code.language-mermaid"))return;
    try{await loadDiagrams();if(root.isConnected)window.HeyBossArtifactDiagrams.mount(root);}
    catch(e){
      if(!root.isConnected)return;
      const note=document.createElement("p");note.className="artifact-muted";note.textContent="Diagrams could not load. The Mermaid source is shown below.";
      root.prepend(note);
    }
  }
  let diagramBundle;
  function loadDiagrams() {
    if(window.HeyBossArtifactDiagrams)return Promise.resolve();
    return diagramBundle ||= new Promise((resolve,reject)=>{
      const script=document.createElement("script");
      script.type="module";
      script.src=mobile?"/artifact-web/artifact-diagrams.js":"/artifact-diagrams.js";
      script.onload=resolve;script.onerror=()=>{diagramBundle=null;script.remove();reject(Error("Could not load diagrams"));};
      document.head.appendChild(script);
    });
  }
  let editorBundle;
  function loadEditor() {
    if(window.HeyBossArtifactEditor)return Promise.resolve();
    return editorBundle ||= new Promise((resolve,reject)=>{
      const script=document.createElement("script");
      script.src=mobile?"/artifact-web/artifact-editor.js":"/artifact-editor.js";
      script.onload=resolve;script.onerror=()=>{editorBundle=null;script.remove();reject(Error("Could not load the writing surface. Reconnect and reopen Edit."));};
      document.head.appendChild(script);
    });
  }
  function mount(root,context) {
    if(!root)return;
    const render = artifacts => {
      root.classList.add("artifact-relationship");
      root.innerHTML=`<h2 class="side-heading">Artifacts</h2><ul class="artifact-attachment-list" ${artifacts.length?"":"hidden"}>${artifacts.map(a=>`<li><a href="${esc(url(context.project,a.id,context.host?{host:context.host}:{}))}">${HeyBossUI.icon("docs")}<span>${esc(a.title)}${a.archived?'<small>Archived</small>':""}</span></a><button type="button" data-unlink="${esc(a.id)}" aria-label="Unlink ${esc(a.title)}" title="Unlink artifact">${HeyBossUI.icon("x")}</button></li>`).join("")}</ul><div class="artifact-attachment-actions"><a class="artifact-text-button" href="${esc(url(context.project,null,{new:"1",...(context.issue?{issue:context.issue}:{node:context.node}),...(context.host?{host:context.host}:{})}))}">${HeyBossUI.icon("plus")}Create artifact</a><button type="button" class="artifact-text-button" data-attach>${HeyBossUI.icon("link")}Attach existing</button></div><div data-attach-form></div><p role="alert" hidden></p>`;
      const error=e=>{const p=$("[role=alert]",root);p.hidden=false;p.textContent=e.message;};
      const target=context.issue?{issue:context.issue}:{node:context.node};
      root.querySelectorAll("[data-unlink]").forEach(b=>b.onclick=async()=>{b.disabled=true;try{await api(context,{command:"unlink",id:b.dataset.unlink,...target});render(artifacts.filter(a=>a.id!==b.dataset.unlink));}catch(e){b.disabled=false;error(e);}});
      $("[data-attach]",root).onclick=async()=>{
        const panel=$("[data-attach-form]",root),actions=$(".artifact-attachment-actions",root);
        panel.innerHTML='<form class="artifact-attachment-picker"><input type="search" aria-label="Find artifact to attach" placeholder="Search documents…"><select aria-label="Artifact to attach" disabled></select><p class="artifact-muted" data-attach-status role="status">Loading documents…</p><div><button class="button" type="submit" disabled>Attach</button><button class="artifact-text-button" type="button" aria-label="Cancel attachment" data-attach-cancel>Cancel</button></div></form>';
        const form=$("form",panel),search=$("input",form),select=$("select",form),button=$("button[type=submit]",form),status=$("[data-attach-status]",form);
        actions.hidden=true;
        let generation=0,timer;
        const cancel=()=>{clearTimeout(timer);generation++;panel.innerHTML="";actions.hidden=false;$("[data-attach]",root).focus();};
        $("[data-attach-cancel]",form).onclick=cancel;
        form.onkeydown=e=>{if(e.key==="Escape"){e.preventDefault();cancel();}};
        const attached=new Set(artifacts.map(a=>a.id));
        const load=async()=>{
          const seq=++generation;select.disabled=true;button.disabled=true;status.textContent="Loading documents…";
          try{const value=await api(context,{command:"list",query:search.value,archived:false});if(!form.isConnected||seq!==generation)return;
            const choices=value.artifacts.filter(a=>!attached.has(a.id));
            select.innerHTML=choices.map(a=>`<option value="${esc(a.id)}">${esc(a.title)}</option>`).join("");
            select.disabled=button.disabled=!choices.length;status.textContent=choices.length?"":search.value.trim()?"No matching artifacts":"No unattached artifacts";
          }catch(err){if(form.isConnected&&seq===generation){status.textContent="";error(err);}}
        };
        search.oninput=()=>{clearTimeout(timer);generation++;select.disabled=true;button.disabled=true;timer=setTimeout(load,250);};
        form.onsubmit=async e=>{
          e.preventDefault();const id=select.value;if(!id||button.disabled)return;button.disabled=true;select.disabled=true;search.disabled=true;
          try{await api(context,{command:"link",id,...target});const value=await api(context,{command:"links",...target});if(form.isConnected)render(value.artifacts);}
          catch(err){if(form.isConnected){button.disabled=false;select.disabled=false;search.disabled=false;error(err);}}
        };
        search.focus();await load();
      };
    };
    render(context.artifacts||[]);
  }

  async function start() {
    if(!$("#artifact-main"))return;
    const skip=$(".skip-link[href='#artifact-main']");
    if(skip)skip.onclick=e=>{e.preventDefault();$("#artifact-main").scrollIntoView({block:"start"});$("#artifact-main").focus({preventScroll:true});};
    $("#artifact-main").addEventListener("keydown",e=>{
      if(e.altKey||e.ctrlKey||e.metaKey||e.shiftKey||!["ArrowLeft","ArrowRight"].includes(e.key))return;
      const code=e.target.closest('pre[tabindex="0"]');
      if(!code||code.scrollWidth<=code.clientWidth)return;
      e.preventDefault();code.scrollLeft+=e.key==="ArrowRight"?40:-40;
    });
    HeyBossUI.icons();
    const route=()=>new URLSearchParams(location.hash.slice(1));
    const status=s=>{if($("#artifact-status").textContent!==s)$("#artifact-status").textContent=s;};
    const icon=name=>HeyBossUI.icon(name);
    const date=value=>new Date(value).toLocaleDateString(undefined,{month:"short",day:"numeric",...(new Date(value).getFullYear()!==new Date().getFullYear()?{year:"numeric"}:{})});
    const libraryURL=()=>url(context.project,null,context.host?{host:context.host}:{});
    function mode(value) {
      selectText=null;writingEditor?.destroy();writingEditor=null;
      $("#artifact-main").dataset.mode=value;
      $("#artifact-heading").hidden=value!=="library";
      $("#artifact-library").hidden=value!=="library";
      $("#artifact-document").hidden=value==="library";
    }
    const error=(e,retry)=>{
      const panel=$("#artifact-error");panel.hidden=false;
      panel.innerHTML=`<p>${esc(e.message)}</p>${retry?'<button class="button" type="button">Try again</button>':""}`;
      if(retry)$("button",panel).onclick=()=>{panel.hidden=true;retry();};
      status(editing?"Could not save · draft preserved":"");
    };
    let boot,context,project,doc=null,editing=false,generation=0,offset=0,rows=[],anchor=null,commentsOpen=null,draftTimer,resizeEditor,resizeFrame,readingHTML,selectText,selectionFrame,writingEditor;
    const editorBody=()=>writingEditor?writingEditor.content():$("#artifact-body").value;
    try {mobile=document.documentElement.dataset.artifactMobile==="true";const r=await fetch(mobile?"/api/artifact-bootstrap":"/api/bootstrap");boot=await r.json();if(!r.ok)throw Error(boot.error?.message||boot.error||"Pair this device to continue");}catch(e){error(e,()=>location.reload());return;}
    const picker=new HeyBossUI.ProjectPicker({onSelect:id=>{location.hash=new URLSearchParams({project:id});}});
    const draftKey=id=>`hey-boss-artifact-draft:${context.host||"local"}:${context.project}:${id||"new"}`;
    const drafts={get:key=>{try{return JSON.parse(localStorage.getItem(key));}catch{return null;}},set:(key,v)=>{try{localStorage.setItem(key,JSON.stringify(v));return true;}catch{return false;}},remove:key=>{try{localStorage.removeItem(key);}catch{}}};
    const activeActions=new Set(),pendingActions=new Map();
    async function documentAction(artifact,command,button,dialog) {
      const key=`${draftKey(artifact.id)}:action`,scope={...context},seq=generation;
      if(activeActions.has(key))return;
      const pending=pendingActions.get(key)||drafts.get(key)||{operation:{command,id:artifact.id,if_version:artifact.version,...(command==="archive"?{archived:!artifact.archived}:{})},requestID:HeyBossUI.requestId()};
      if(pending.operation.command!==command){error(Error("An earlier action is pending. Retry that action before making another change."));dialog?.close();return;}
      pendingActions.set(key,pending);drafts.set(key,pending);activeActions.add(key);button.disabled=true;
      if(dialog){$("[role=alert]",dialog).hidden=true;$("[data-cancel]",dialog).disabled=true;dialog.oncancel=e=>e.preventDefault();}
      try {
        const result=await api(scope,pending.operation,pending.requestID);
        pendingActions.delete(key);drafts.remove(key);
        if(command==="delete"){
          const prefix=key.slice(0,-7);
          try{for(const stored of Object.keys(localStorage))if(stored===prefix||stored.startsWith(prefix+":"))drafts.remove(stored);}catch{}
        }
        dialog?.close();
        if(seq!==generation)return;
        $("#artifact-error").hidden=true;
        if(doc?.id===artifact.id){
          if(command==="delete"){location.href=libraryURL();return;}
          doc=result.artifact;doc.result=result;reading();
        }else{
          await library();
          if(context.project!==scope.project||context.host!==scope.host||doc||editing)return;
          ($(".artifact-row-link")||$("#artifact-empty-action")||$("#artifact-search"))?.focus({preventScroll:true});
        }
        status(command==="delete"?"Artifact permanently deleted":pending.operation.archived?"Artifact archived":"Artifact restored");
      }catch(e){
        if(!e.uncertain){pendingActions.delete(key);drafts.remove(key);}
        if(seq!==generation)return;
        const message=e.uncertain?"Delivery is unconfirmed. Retry this same action to confirm the result.":e.message;
        if(dialog?.open){const panel=$("[role=alert]",dialog);panel.textContent=message;panel.hidden=false;button.textContent=e.uncertain?"Retry deletion":e.code==="conflict"?"Reload latest":"Delete permanently";if(e.code==="conflict")button.onclick=()=>{dialog.close();navigate();};}
        else error(Error(message),e.code==="conflict"?navigate:()=>documentAction(artifact,command,button));
      }finally{activeActions.delete(key);button.disabled=false;if(dialog){$("[data-cancel]",dialog).disabled=false;dialog.oncancel=null;}}
    }
    function confirmDelete(artifact,trigger) {
      if($("#artifact-delete-dialog"))return;
      const dialog=document.createElement("dialog");dialog.id="artifact-delete-dialog";dialog.className="artifact-delete-dialog";
      dialog.setAttribute("aria-labelledby","artifact-delete-title");dialog.setAttribute("aria-describedby","artifact-delete-description");
      dialog.innerHTML=`<h2 id="artifact-delete-title">Delete artifact permanently?</h2><p class="artifact-delete-name">${esc(artifact.title)}</p><p id="artifact-delete-description">This removes the document, comments, attached files, and links from issues and mindmaps. This cannot be undone.</p><p class="artifact-delete-hint">To keep it for later, use Archive instead.</p><p role="alert" hidden></p><div class="artifact-delete-actions"><button class="button" type="button" data-cancel autofocus>Cancel</button><button class="button danger" type="button" data-confirm>Delete permanently</button></div>`;
      document.body.append(dialog);
      $("[data-cancel]",dialog).onclick=()=>dialog.close();
      $("[data-confirm]",dialog).onclick=e=>documentAction(artifact,"delete",e.currentTarget,dialog);
      dialog.onclose=()=>{dialog.remove();if(trigger.isConnected)trigger.focus();};
      trigger.focus({preventScroll:true});
      dialog.showModal();
    }
    function keepDraft() {
      clearTimeout(draftTimer);
      if(!editing)return;
      const key=draftKey(doc?.id),value={title:$("#artifact-title").value,body:editorBody(),version:Number($("#artifact-editor").dataset.version),requestID:$("#artifact-editor").dataset.requestId,pending:$("#artifact-editor").dataset.pending?JSON.parse($("#artifact-editor").dataset.pending):null};
      value.files=JSON.parse($("#artifact-editor").dataset.importFiles||"[]");
      resizeEditor?.(value.body.length);
      const saved=drafts.set(key,value);status(saved?"Draft saved in this browser":"Draft is only in this tab · browser storage unavailable");
    }
    function editor() {
      editing=true;
      const draft=drafts.get(draftKey(doc?.id));
      mode("editor");
      document.title=`${doc?"Edit "+doc.title:"New artifact"} · Hey Boss`;
      $("#artifact-document").innerHTML=`<form id="artifact-editor" class="artifact-editor" data-version="${draft?.version||doc?.version||1}" data-request-id="${esc(draft?.requestID||HeyBossUI.requestId())}">
        <header class="artifact-editor-toolbar"><button class="artifact-text-button" type="button" id="artifact-cancel">${icon("arrow-left")}Back</button><h1>${doc?"Edit artifact":"New artifact"}</h1><div class="artifact-editor-tools"><button class="button" type="button" id="artifact-preview" aria-pressed="false">Preview</button><button class="button artifact-import" type="button" id="artifact-import-open" title="Choose Markdown and its linked files together">Import</button><input id="artifact-import" type="file" multiple hidden><button class="button primary" type="submit">Save</button></div></header>
        <div class="artifact-editor-paper"><label class="artifact-title-label" for="artifact-title">Title</label><input id="artifact-title" aria-label="Title" required maxlength="512" placeholder="Untitled artifact" value="${esc(draft?.title??doc?.title??"")}"><div id="artifact-write"><label class="artifact-body-label" for="artifact-body">Markdown</label><textarea id="artifact-body" placeholder="Start writing in Markdown…" spellcheck="true">${esc(draft?.body??doc?.body??"")}</textarea></div><div class="markdown artifact-reading" id="artifact-edit-preview" hidden></div></div><div id="artifact-conflict" class="artifact-conflict"></div></form>`;
      let saving=false,pending=draft?.pending||null;
      $("#artifact-editor").dataset.importFiles=JSON.stringify(draft?.files||[]);
      if(pending){$("#artifact-editor").dataset.pending=JSON.stringify(pending);$("#artifact-title").disabled=true;$("#artifact-body").disabled=true;$("#artifact-import").disabled=true;$("#artifact-import-open").disabled=true;}
      const autosize=length=>{
        const body=$("#artifact-body");if(!body||body.closest('[hidden]'))return;
        if((length??body.value.length)>32000){body.style.fieldSizing="fixed";if(body.style.height!=="65vh")body.style.height="65vh";return;}
        if(CSS.supports("field-sizing","content")){body.style.fieldSizing="content";body.style.height="auto";return;}
        const position=scrollY;
        // Large files retain a bounded writing viewport instead of expanding the page.
        body.style.height="auto";
        body.style.height=body.scrollHeight+"px";
        if(scrollY!==position)scrollTo(0,position);
      };
      resizeEditor=autosize;
      const changed=()=>{
        if(!saving&&!pending){$("#artifact-editor").dataset.requestId=HeyBossUI.requestId();pending=null;}
        const body=$("#artifact-body");if(!writingEditor&&body.style.height!=="65vh")autosize();
        status("Editing…");clearTimeout(draftTimer);draftTimer=setTimeout(keepDraft,300);
      };
      for(const input of [$("#artifact-title"),$("#artifact-body")])input.oninput=changed;
      const initialBody=draft?.body??doc?.body??"";
      if(initialBody.length>32000){
        const form=$("#artifact-editor");
        loadEditor().then(()=>{
          if(!form.isConnected)return;
          const host=document.createElement("div");host.id="artifact-code-editor";
          const body=$("#artifact-body");host.className="artifact-code-editor";body.before(host);
          writingEditor=window.HeyBossArtifactEditor(host,body.value,changed);body.hidden=true;
          writingEditor.readOnly(!!pending||saving);
        }).catch(err=>{if(form.isConnected)error(err);});
      }
      $("#artifact-cancel").onclick=()=>{keepDraft();editing=false;doc?reading():(()=>{
        const params=route(),issue=params.get("issue"),node=params.get("node");
        if(issue||node)location.href=resourceURL(context,issue?"issue":"node",issue||node);
        else location.href=libraryURL();
      })();};
      $("#artifact-import").onchange=async e=>{
        const selected=[...e.target.files],form=$("#artifact-editor");e.target.value="";
        if(!selected.length)return;
        const markdown=selected.filter(file=>/\.(md|markdown)$/i.test(file.name));
        if(markdown.length!==1){error(Error("Choose one Markdown document and its linked files together."));return;}
        const file=markdown[0];if(file.size>1048576){error(Error("Markdown must be at most 1 MiB"));return;}
        saving=true;form.querySelectorAll("button,input,textarea").forEach(el=>el.disabled=true);
        try {
          const body=new TextDecoder("utf-8",{fatal:true}).decode(await file.arrayBuffer()),preview=await api(context,{command:"preview",body}),files=[];
          let total=0;
          for(const destination of preview.local_files||[]) {
            const name=decodeURIComponent(destination.split(/[?#]/)[0]).split("/").pop();
            const matches=selected.filter(file=>file.name===name);
            if(matches.length!==1)throw Error("Select the linked file “"+name+"” with your Markdown document, then import again.");
            const attachment=matches[0];total+=attachment.size;
            if(total>10*1024*1024)throw Error("Markdown attachments must total at most 10 MB.");
            const data=await new Promise((resolve,reject)=>{const reader=new FileReader();reader.onload=()=>resolve(reader.result.split(",")[1]);reader.onerror=()=>reject(Error("Could not read "+name));reader.readAsDataURL(attachment);});
            files.push({destination,name,data});
          }
          if(!form.isConnected)return;
          $("#artifact-error").hidden=true;
          form.dataset.importFiles=JSON.stringify(files);
          if(writingEditor)writingEditor.replace(body);else $("#artifact-body").value=body;
          changed();keepDraft();status(files.length?"Imported Markdown and "+files.length+" linked files. Save to upload.":"Markdown imported. Save when ready.");
        } catch(e){error(e);} finally {saving=false;if(form.isConnected)form.querySelectorAll("button,input,textarea").forEach(el=>el.disabled=false);}
      };
      $("#artifact-import-open").onclick=()=>$("#artifact-import").click();
      let previewing=false,previewGeneration=0;
      $("#artifact-preview").onclick=async()=>{
        const button=$("#artifact-preview"),panel=$("#artifact-edit-preview"),write=$("#artifact-write");
        if(previewing){previewGeneration++;previewing=false;panel.hidden=true;write.hidden=false;button.textContent="Preview";button.setAttribute("aria-pressed","false");autosize();writingEditor?writingEditor.focus():$("#artifact-body").focus();return;}
        const seq=++previewGeneration;button.disabled=true;
        try{const v=await api(context,{command:"preview",body:editorBody()});if(!panel.isConnected||seq!==previewGeneration)return;previewing=true;panel.hidden=false;write.hidden=true;renderMarkdown(panel,v.html||'<p class="artifact-muted">Your document preview will appear here.</p>',context,JSON.parse($("#artifact-editor").dataset.importFiles||"[]"));button.textContent="Write";button.setAttribute("aria-pressed","true");}
        catch(e){error(e);}finally{if(button.isConnected)button.disabled=false;}
      };
      $("#artifact-editor").onsubmit=async e=>{
        e.preventDefault();if(saving)return;keepDraft();const form=e.currentTarget,key=draftKey(doc?.id),title=$("#artifact-title").value,body=editorBody();
        const target=route().get("issue")?{issue:Number(route().get("issue"))}:route().get("node")?{node:route().get("node")}:{};
        saving=true;writingEditor?.readOnly(true);form.querySelectorAll("input,textarea,button").forEach(el=>el.disabled=true);status("Saving…");$("#artifact-error").hidden=true;
        try {
          if(!pending){
            const candidates=JSON.parse(form.dataset.importFiles||"[]"),preview=await api(context,{command:"preview",body}),files=[];
            for(const destination of preview.local_files||[]){
              const file=candidates.find(file=>file.destination===destination);
              if(!file)throw Error("Import your Markdown with its linked files before saving: "+destination);
              files.push(file);
            }
            const operation=doc?{command:"edit",id:doc.id,title,body,if_version:Number(form.dataset.version)}:{command:"create",title,body,...target};
            pending=files.length?{command:"import",operation,files}:operation;
            form.dataset.pending=JSON.stringify(pending);keepDraft();
          }
          const v=await api(context,pending,form.dataset.requestId);drafts.remove(key);if(!form.isConnected)return;doc=v.artifact;doc.result=v;editing=false;status("Saved");location.hash=new URLSearchParams({project:context.project,artifact:doc.id,...(context.host?{host:context.host}:{})});reading();}
        catch(err){if(!form.isConnected)return;error(err);if(!err.uncertain){pending=null;form.dataset.pending="";form.dataset.requestId=HeyBossUI.requestId();keepDraft();}else{status("Save pending · retry with the same content to avoid duplicate documents");}if(doc&&err.code==="conflict"){ $("#artifact-conflict").innerHTML='<button class="button" id="artifact-load-latest" type="button">Load latest for comparison</button>';$("#artifact-load-latest").onclick=async()=>{try{const latest=await api(context,{command:"view",id:doc.id});if(!form.isConnected)return;const panel=$("#artifact-conflict");panel.innerHTML=`<h3>Latest saved revision ${latest.artifact.version}</h3><strong>${esc(latest.artifact.title)}</strong><pre>${esc(latest.artifact.body)}</pre><p>Merge the saved changes into your draft above, then use this revision to save.</p><button type="button" class="button" id="artifact-use-revision">Use revision ${latest.artifact.version} for merged draft</button>`;$("#artifact-use-revision").onclick=()=>{form.dataset.version=latest.artifact.version;form.dataset.requestId=HeyBossUI.requestId();pending=null;keepDraft();panel.innerHTML="<p>Latest revision selected. Review your merged draft and Save.</p>";};}catch(e){error(e);}};}}
        finally {saving=false;if(form.isConnected){writingEditor?.readOnly(!!pending);form.querySelectorAll("input,textarea,button").forEach(el=>el.disabled=false);if(pending){$("#artifact-title").disabled=true;$("#artifact-body").disabled=true;$("#artifact-import").disabled=true;$("#artifact-import-open").disabled=true;}}}
      };
      $("#artifact-editor").onkeydown=e=>{if((e.metaKey||e.ctrlKey)&&e.key.toLowerCase()==="s"){e.preventDefault();if(!saving)e.currentTarget.requestSubmit();}};
      autosize((draft?.body??doc?.body??"").length);status(pending?"Recovered pending save · retry to confirm delivery":draft?"Draft restored":"Changes save as a browser draft");$("#artifact-title").focus();
    }
    function reading() {
      editing=false;anchor=null;status("");const v=doc.result;
      mode("reading");
      const content=doc.body_html||'<p class="artifact-muted">This document is empty. Choose Edit to start writing.</p>';
      const previous=$("#artifact-reading"),reuse=previous&&previous.dataset.artifact===doc.id&&readingHTML===content;
      const comment=c=>`<p class="artifact-comment-meta"><strong title="${esc(c.author)}">${esc(author(c.author))}</strong><time datetime="${esc(c.created_at)}" title="${esc(new Date(c.created_at).toLocaleString())}">${esc(date(c.created_at))}</time></p><div class="markdown">${c.body_html}</div>`;
      const replies=new Map();
      for(const c of v.comments)if(c.parent){if(!replies.has(c.parent))replies.set(c.parent,[]);replies.get(c.parent).push(c);}
      const threads=v.comments.filter(c=>!c.parent).map(c=>{
        const body=`${c.quote?`<blockquote>${esc(c.quote)}</blockquote>${c.outdated?'<p class="artifact-muted">Outdated selection · discussion preserved</p>':""}`:""}${comment(c)}${(replies.get(c.id)||[]).map(r=>`<div class="reply">${comment(r)}</div>`).join("")}<div class="artifact-thread-actions"><details class="artifact-reply"><summary>Reply</summary><form data-reply="${c.id}"><textarea aria-label="Reply to comment" required placeholder="Write a reply…" rows="3">${esc(drafts.get(`${draftKey(doc.id)}:reply:${c.id}`)?.body||"")}</textarea><button class="button" type="submit">Reply</button></form></details><button class="artifact-text-button" type="button" data-resolve="${c.id}" data-resolved="${!c.resolved}">${c.resolved?"Reopen thread":"Resolve"}</button></div>`;
        return c.resolved?`<details class="artifact-thread resolved"><summary>Resolved · ${esc(c.body.slice(0,70))}</summary>${body}</details>`:`<section class="artifact-thread">${body}</section>`;
      }).join("");
      const backlinks=v.backlinks.map(l=>`<li>${l.title?`<a href="${esc(resourceURL(context,l.kind,l.target))}">${esc(l.title)}${l.kind==="issue"?` · #${esc(l.target)}`:""}</a>`:`<span class="artifact-muted">Removed ${esc(l.kind)} · ${esc(l.target)}</span>`}</li>`).join("");
      const commentCount=v.comments.filter(c=>!c.parent&&!c.resolved).length;
      commentsOpen ??= false;
      $("#artifact-document").innerHTML=`<a class="back-link" href="${esc(libraryURL())}">${icon("arrow-left")}All artifacts</a>
        <header class="artifact-document-heading"><div><h1>${esc(doc.title)}</h1><p class="artifact-muted">Updated ${esc(date(doc.updated_at))} · Revision ${doc.version}${doc.archived?' <span class="artifact-badge">Archived</span>':""}</p></div><div class="artifact-actions"><button class="button" id="artifact-comments-toggle" aria-expanded="${commentsOpen}" aria-controls="artifact-comments">${icon("comment")}Comments${commentCount?` <span class="artifact-count">${commentCount}</span>`:""}</button><button class="button primary" id="artifact-edit">${icon("edit")}Edit</button><details class="artifact-menu"><summary class="button" aria-label="More actions"><span aria-hidden="true">•••</span><span class="artifact-sr-only">More actions</span></summary><div class="artifact-menu-panel"><button id="artifact-export" type="button">${icon("docs")}Export Markdown</button><button id="artifact-archive" type="button">${icon(doc.archived?"refresh":"archive")}${doc.archived?"Restore":"Archive"}</button><button id="artifact-delete" class="artifact-danger" type="button">${icon("trash")}Delete permanently…</button></div></details></div></header>
        <button class="button primary artifact-selection-action" id="artifact-selection-comment" type="button" aria-label="Comment on selection" hidden>${icon("comment")}Comment on selection</button>
        <div class="artifact-layout${commentsOpen?"":" comments-hidden"}"><div class="artifact-content"><article id="artifact-reading" class="markdown artifact-reading"></article><section id="artifact-attachments"></section>${HeyBossOrigin.card(doc.origin,context.project,author)}${backlinks?`<section class="artifact-backlinks"><h2>Linked from</h2><ul class="artifact-links">${backlinks}</ul></section>`:""}</div><aside id="artifact-comments" aria-label="Document comments" ${commentsOpen?"":"hidden"}><div class="artifact-comments-heading"><h2>Comments</h2><button class="artifact-text-button" type="button" id="artifact-comments-close" aria-label="Close comments">${icon("x")}</button></div><p class="artifact-muted artifact-comment-hint">Select a passage to comment on it.</p><form id="artifact-comment-form"><blockquote id="artifact-quote" class="artifact-quote" hidden></blockquote><button class="artifact-text-button" type="button" id="artifact-clear-quote" hidden>Clear selection</button><label class="artifact-sr-only" for="artifact-comment">Add a comment</label><textarea id="artifact-comment" required rows="3" placeholder="Add a comment…"></textarea><button class="button primary" type="submit">Comment</button></form><div id="artifact-threads">${threads}</div></aside></div>`;
      if(reuse)$("#artifact-reading").replaceWith(previous);
      else{const reader=$("#artifact-reading");reader.dataset.artifact=doc.id;renderMarkdown(reader,content,context);}
      externalLinks($("#artifact-threads"));
      HeyBossAttachments.mount($("#artifact-attachments"), {...context,target:{kind:"artifact",id:doc.id}});
      readingHTML=content;
      const showComments=open=>{commentsOpen=open;$("#artifact-comments").hidden=!open;$(".artifact-layout").classList.toggle("comments-hidden",!open);$("#artifact-comments-toggle").setAttribute("aria-expanded",String(open));};
      $("#artifact-comments-toggle").onclick=()=>{
        showComments(!commentsOpen);
        if(commentsOpen&&matchMedia("(max-width: 900px)").matches)$("#artifact-comments").scrollIntoView({behavior:matchMedia("(prefers-reduced-motion: reduce)").matches?"instant":"smooth",block:"start"});
      };
      $("#artifact-comments-close").onclick=()=>{showComments(false);if(matchMedia("(max-width: 900px)").matches)$(".artifact-document-heading").scrollIntoView({block:"start"});$("#artifact-comments-toggle").focus({preventScroll:true});};
      const menu=$(".artifact-menu");
      menu.onkeydown=e=>{if(e.key==="Escape"){menu.open=false;$("summary",menu).focus();}};
      document.title=`${doc.title} · Artifacts · Hey Boss`;
      $("#artifact-edit").onclick=editor;
      $("#artifact-export").onclick=()=>{menu.open=false;const blob=new Blob([doc.body],{type:"text/markdown;charset=utf-8"}),a=document.createElement("a");a.href=URL.createObjectURL(blob);a.download=(doc.title.replace(/[^\p{L}\p{N}_ -]/gu,"_")||"artifact")+".md";a.click();setTimeout(()=>URL.revokeObjectURL(a.href),1000);};
      const mutate=async(op,button)=>{button.disabled=true;try{const r=await api(context,op);if(!button.isConnected)return;doc=r.artifact;doc.result=r;reading();status("Saved");}catch(e){button.disabled=false;error(e);}};
      $("#artifact-archive").onclick=e=>documentAction(doc,"archive",e.currentTarget);
      $("#artifact-delete").onclick=e=>{menu.open=false;confirmDelete(doc,$("summary",menu));};
      $("#artifact-document").querySelectorAll("[data-resolve]").forEach(b=>b.onclick=()=>mutate({command:"resolve",id:doc.id,comment_id:Number(b.dataset.resolve),resolved:b.dataset.resolved==="true"},b));
      $("#artifact-document").querySelectorAll("[data-reply]").forEach(form=>{
        const field=$("textarea",form),button=$("button",form),key=`${draftKey(doc.id)}:reply:${form.dataset.reply}`;
        let pending=drafts.get(key)?.pending||null;
        field.readOnly=!!pending;
        const keep=()=>drafts.set(key,{body:field.value,pending});
        field.oninput=keep;
        form.onsubmit=async e=>{
          e.preventDefault();if(button.disabled||!field.value.trim())return;
          button.disabled=true;
          pending ||= {operation:{command:"comment",id:doc.id,parent:Number(form.dataset.reply),body:field.value},requestID:HeyBossUI.requestId()};
          field.readOnly=true;keep();
          try{const r=await api(context,pending.operation,pending.requestID);drafts.remove(key);if(!form.isConnected)return;doc=r.artifact;doc.result=r;reading();status("Reply saved");}
          catch(err){if(!form.isConnected)return;button.disabled=false;if(!err.uncertain){pending=null;field.readOnly=false;keep();}error(err);if(err.uncertain)status("Reply pending · retry after reconnecting");}
        };
      });
      const commentKey=`${draftKey(doc.id)}:comment`;
      let pendingComment=drafts.get(commentKey)?.pending||null;
      $("#artifact-comment").value=drafts.get(commentKey)?.body||"";$("#artifact-comment").readOnly=!!pendingComment;
      $("#artifact-comment").oninput=()=>drafts.set(commentKey,{body:$("#artifact-comment").value,anchor,pending:pendingComment});
      const restored=drafts.get(commentKey)?.anchor;
      const showAnchor=()=>{if(anchor)showComments(true);$("#artifact-quote").hidden=!anchor;$("#artifact-quote").textContent=anchor?.quote||"";$("#artifact-clear-quote").hidden=!anchor;};
      if(restored){anchor=restored;showAnchor();}
      $("#artifact-clear-quote").onclick=()=>{if(pendingComment)return;anchor=null;showAnchor();$("#artifact-comment").dispatchEvent(new Event("input"));};
      // A selection is also used for copying. Keep the reader stable until the
      // explicit Comment action is chosen, including touch/keyboard selections.
      const selectionButton=$("#artifact-selection-comment");
      let candidate=null;
      const select=()=>{
        const selection=getSelection(),reader=$("#artifact-reading");
        if(document.activeElement===selectionButton)return;
        candidate=null;selectionButton.hidden=true;
        if(!selection.rangeCount||selection.isCollapsed||pendingComment||reader.hasAttribute("aria-busy"))return;
        const range=selection.getRangeAt(0);
        if(!reader.contains(range.commonAncestorContainer))return;
        for(const node of [range.startContainer,range.endContainer]){
          const diagram=(node.nodeType===1?node:node.parentElement)?.closest(".artifact-diagram");
          if(diagram&&!diagram.querySelector("pre").contains(node))return;
        }
        const quote=selection.toString();
        if(!quote.trim()||new TextEncoder().encode(quote).length>8192)return;
        const before=range.cloneRange();before.selectNodeContents(reader);before.setEnd(range.startContainer,range.startOffset);
        const after=range.cloneRange();after.selectNodeContents(reader);after.setStart(range.endContainer,range.endOffset);
        candidate={quote,prefix:Array.from(anchorText(before).slice(-160)).slice(-80).join(""),suffix:Array.from(anchorText(after).slice(0,160)).slice(0,80).join("")};
        selectionButton.hidden=false;
        if(!matchMedia("(max-width: 900px)").matches){
          const rect=range.getBoundingClientRect();
          selectionButton.style.left=Math.max(12,Math.min(rect.left,innerWidth-selectionButton.offsetWidth-12))+"px";
          selectionButton.style.top=Math.max(12,Math.min(rect.bottom+8,innerHeight-selectionButton.offsetHeight-12))+"px";
        }
      };
      // Preserve mouse selections without suppressing WebKit's touch click.
      selectionButton.onpointerdown=e=>{if(e.pointerType!=="touch")e.preventDefault();};
      selectionButton.onclick=()=>{
        if(!candidate)return;
        anchor=candidate;candidate=null;selectionButton.hidden=true;showAnchor();
        $("#artifact-comment").dispatchEvent(new Event("input"));
        // The sidebar can be above a passage selected in a long document.
        // Bring the quoted composer into view before moving keyboard focus.
        $("#artifact-comment-form").scrollIntoView({block:"center"});
        $("#artifact-comment").focus();
        getSelection().removeAllRanges();
      };
      selectText=select;$("#artifact-reading").onpointerup=select;$("#artifact-reading").onkeyup=select;
      $("#artifact-reading").onclick=e=>{
        const reader=e.currentTarget,link=e.target.closest('a[href^="#"]');
        if(!link||e.defaultPrevented)return;
        let id=link.getAttribute("href").slice(1),decoded=id;
        try{decoded=decodeURIComponent(id);}catch{}
        const target=reader.querySelector("#"+CSS.escape(id))||reader.querySelector("#"+CSS.escape(decoded));
        if(!target)return;
        e.preventDefault();target.scrollIntoView({block:"start"});
        if(!target.hasAttribute("tabindex")){target.tabIndex=-1;target.addEventListener("blur",()=>target.removeAttribute("tabindex"),{once:true});}
        target.focus({preventScroll:true});
      };
      $("#artifact-comment-form").onsubmit=async e=>{e.preventDefault();const b=$("button[type=submit]",e.currentTarget);b.disabled=true;pendingComment ||= {operation:{command:"comment",id:doc.id,body:$("#artifact-comment").value,...(anchor||{})},requestID:HeyBossUI.requestId()};$("#artifact-comment").readOnly=true;$("#artifact-comment").dispatchEvent(new Event("input"));try{const r=await api(context,pendingComment.operation,pendingComment.requestID);drafts.remove(commentKey);if(!b.isConnected)return;doc=r.artifact;doc.result=r;reading();status("Comment saved");}catch(e){if(!b.isConnected)return;b.disabled=false;if(!e.uncertain){pendingComment=null;$("#artifact-comment").readOnly=false;$("#artifact-comment").dispatchEvent(new Event("input"));}error(e);}};
    }
    async function library(append=false) {
      const seq=++generation;status("Loading…");$("#artifact-error").hidden=true;$("#artifact-more").hidden=true;
      const list=$("#artifact-list");list.setAttribute("aria-busy","true");
      if(!list.children.length)list.innerHTML='<div class="artifact-loading" aria-hidden="true"><span></span><span></span><span></span></div>';
      try {const value=await api(context,{command:"list",query:$("#artifact-search").value,archived:$("#artifact-archived").checked,offset:append?offset:0});if(seq!==generation)return;rows=append?[...rows,...value.artifacts]:value.artifacts;offset=rows.length;
        const query=$("#artifact-search").value.trim(),archived=$("#artifact-archived").checked;
        $("#artifact-new").hidden=!rows.length&&!query;
        $("#artifact-list").innerHTML=rows.map(a=>`<div class="artifact-row"><a class="artifact-row-link" href="${esc(url(context.project,a.id,context.host?{host:context.host}:{}))}"><span class="artifact-document-icon">${icon("docs")}</span><span class="artifact-row-content"><strong>${esc(a.title)}</strong><span class="artifact-row-meta">${a.archived?'<span class="artifact-badge">Archived</span>':""}Updated ${esc(date(a.updated_at))}</span></span></a><div class="artifact-row-actions"><button class="icon-button" type="button" data-archive="${esc(a.id)}" aria-label="${a.archived?"Restore":"Archive"} ${esc(a.title)}" title="${a.archived?"Restore":"Archive"}">${icon(a.archived?"refresh":"archive")}</button><button class="icon-button artifact-danger" type="button" data-delete="${esc(a.id)}" aria-label="Delete ${esc(a.title)} permanently" title="Delete permanently…">${icon("trash")}</button></div></div>`).join("")||`<div class="artifact-empty"><span class="artifact-empty-icon">${icon(query?"search":"docs")}</span><h2>${query?"No matching artifacts":archived?"No archived artifacts":"Your documents start here"}</h2><p>${query?"Try a different title or a phrase from the document.":archived?"Archived documents appear here. Restore them whenever you need them.":"Keep plans, notes, and decisions together in your project."}</p><button class="button${query?"":" primary"}" type="button" id="artifact-empty-action">${query?"Clear search":archived?"Show active artifacts":"New artifact"}</button></div>`;
        list.querySelectorAll("[data-archive]").forEach(b=>b.onclick=()=>documentAction(rows.find(a=>a.id===b.dataset.archive),"archive",b));
        list.querySelectorAll("[data-delete]").forEach(b=>b.onclick=()=>confirmDelete(rows.find(a=>a.id===b.dataset.delete),b));
        if($("#artifact-empty-action"))$("#artifact-empty-action").onclick=()=>{if(query){$("#artifact-search").value="";$("#artifact-search").focus();library();}else if(archived){$("#artifact-archived").checked=false;library();}else $("#artifact-new").click();};
        $("#artifact-more").hidden=!value.more;status(`${rows.length} ${rows.length===1?"document":"documents"}${value.more?" · more available":""}${query?" found":" · recently updated"}`);
      }catch(e){if(seq===generation){if($(".artifact-loading",list))list.replaceChildren();error(e,()=>library(append));}}
      finally{if(seq===generation)list.removeAttribute("aria-busy");}
    }
    async function navigate() {
      $("#artifact-delete-dialog")?.close();
      clearTimeout(timer);if(editing)keepDraft();editing=false;generation++;$("#artifact-error").hidden=true;
      const params=route(),id=HeyBossUI.projectId(boot.projects[0]?.id);project=boot.projects.find(p=>p.id===id)||boot.projects[0];
      if(!project){error(Error("No registered projects. Reconnect the supervisor to load project data."),()=>location.reload());return;}
      context={project:project.id,csrf:boot.csrf,host:params.get("host")};for(const input of [$("#artifact-new"),$("#artifact-search"),$("#artifact-archived")])input.disabled=false;picker.update(boot.projects,project);doc=null;commentsOpen=null;
      $("#nav-artifacts").setAttribute("aria-current","page");
      if(mobile){$("#quick-issue-open").hidden=true;$("#nav-inbox").href="/";$("#nav-issues").href="/#issues";$("#nav-workers").hidden=true;$("#nav-mindmaps").hidden=true;}
      const resource=HeyBossRoutes.resolve();const idDoc=resource?.entity==="artifact"?resource.id:"";
      if(idDoc){mode("reading");$("#artifact-document").innerHTML='<div class="artifact-loading artifact-loading-document" aria-hidden="true"><span></span><span></span><span></span></div>';const seq=++generation;status("Loading…");try{const v=await api(context,{command:"view",id:idDoc});if(seq!==generation)return;doc=v.artifact;doc.result=v;reading();status("");}catch(e){if(seq===generation){$("#artifact-document").innerHTML=`<a class="back-link" href="${esc(libraryURL())}">${icon("arrow-left")}All artifacts</a>`;error(e,navigate);}}}
      else if(params.get("new")==="1")editor();
      else {mode("library");document.title="Artifacts · Hey Boss";await library();}
    }
    $("#artifact-new").onclick=()=>{location.hash=new URLSearchParams({project:context.project,new:"1",...(context.host?{host:context.host}:{})});};
    let timer;$("#artifact-search").oninput=()=>{generation++;$("#artifact-more").hidden=true;clearTimeout(timer);timer=setTimeout(()=>library(),250);};
    $("#artifact-archived").onchange=()=>library();$("#artifact-more").onclick=()=>library(true);
    document.addEventListener("pointerdown",e=>{const menu=$(".artifact-menu[open]");if(menu&&!menu.contains(e.target))menu.open=false;});
    window.addEventListener("resize",()=>{cancelAnimationFrame(resizeFrame);if(editing)resizeFrame=requestAnimationFrame(()=>{if(editing)resizeEditor();});});
    document.addEventListener("selectionchange",()=>{cancelAnimationFrame(selectionFrame);selectionFrame=requestAnimationFrame(()=>selectText?.());});
    document.addEventListener("scroll",()=>{const button=$("#artifact-selection-comment");if(button)button.hidden=true;},{passive:true});
    window.addEventListener("hashchange",navigate);window.addEventListener("beforeunload",()=>keepDraft());
    await navigate();
  }
  let resourceGeneration=0,resourcePicker,resourceStatusTimer;
  window.addEventListener("pagehide",()=>clearInterval(resourceStatusTimer));
  window.addEventListener("pageshow",event=>{if(event.persisted&&$("#resource-main"))startResource();});
  async function startResource() {
    if(!$("#resource-main"))return;
    const ticket=++resourceGeneration;
    clearInterval(resourceStatusTimer);
    mobile=true;HeyBossUI.icons();
    $("#resource-status").textContent="Loading project resource…";
    $("#resource-content").replaceChildren();$("#resource-artifacts").replaceChildren();
    const params=new URLSearchParams(location.hash.slice(1));
    try {
      const response=await fetch("/api/artifact-bootstrap"),boot=await response.json();
      if(ticket!==resourceGeneration)return;
      if(!response.ok)throw Error(boot.error||"Pair this device to continue");
      const project=boot.projects.find(p=>p.id===params.get("project"));if(!project)throw Error("Project is unavailable");
      resourcePicker ||= new HeyBossUI.ProjectPicker({onSelect:id=>location.href=url(id)});
      resourcePicker.update(boot.projects,project);
      $("#quick-issue-open").hidden=true;$("#nav-inbox").href="/";$("#nav-issues").href="/#issues";$("#nav-workers").hidden=true;$("#nav-mindmaps").hidden=true;
      const context={project:project.id};
      const resolved=HeyBossRoutes.resolve();const issue=resolved?.entity==="issue"?resolved.id:null,node=resolved?.entity==="node"?resolved.id:null;
      const result=await rpc(context,issue?{action:"view",number:Number(issue)}:{action:"mindmap",operation:{command:"view",node,body_mode:"full"}},true);
      if(ticket!==resourceGeneration)return;
      const resource=issue?result.issue:result.nodes.find(n=>n.id===node)||result.nodes[0];
      if(!resource)throw Error("Resource was removed");
      if(!resource.body_html){resource.body_html=(await api(context,{command:"preview",body:resource.body||""})).html;}
      const links=await api(context,issue?{command:"links",issue:Number(issue)}:{command:"links",node:resource.id});
      if(ticket!==resourceGeneration)return;
      document.title=resource.title+" · Hey Boss";
      $("#resource-content").innerHTML=`<h1>${esc(resource.display_label||resource.title)}</h1><p class="artifact-muted">${issue?`Issue #${esc(issue)} · ${esc(resource.state)}`:"Mindmap topic"}</p><article class="markdown artifact-reading">${resource.body_html||esc(resource.body)}</article>${issue?HeyBossStatus.card(resource,author)+HeyBossOrigin.card(resource.origin,context.project,author):""}<section id="resource-attachments"></section>`;
      if(issue){
        const attach=()=>HeyBossStatus.mount($(".issue-progress-card"),resource,author,(offset,before)=>rpc(context,{action:"status_history",number:Number(issue),limit:20,offset,before},true));
        let update=attach(),refreshing=false;
        resourceStatusTimer=setInterval(async()=>{
          if(document.hidden||refreshing)return;
          refreshing=true;
          try{
            const result=await rpc(context,{action:"status_view",number:Number(issue)},true);
            if(ticket!==resourceGeneration)return;
            const next={...resource,status:result.status,assignee:result.assignee};
            if(update(next)==="remount"){Object.assign(resource,next);update=attach();}
            else Object.assign(resource,next);
          }catch{/* Keep the last update and its original timestamp while offline. */}
          finally{refreshing=false;}
        },15000);
      }
      HeyBossAttachments.mount($("#resource-attachments"),{...context,target:{kind:issue?"issue":"node",id:issue?String(issue):resource.id},readonly:!!resource.deleted_at});
      mount($("#resource-artifacts"),{...context,...(issue?{issue:Number(issue)}:{node:resource.id}),artifacts:links.artifacts});
      $("#resource-status").textContent="";
    }catch(e){if(ticket===resourceGeneration)$("#resource-status").textContent=e.message;}
  }
  if($("#resource-main"))window.addEventListener("hashchange",startResource);
  startResource();
  start();
  return {mount,url,rpc};
})();
