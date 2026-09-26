"use strict";
const HeyBossAttachments = (() => {
  const limit = 10 * 1024 * 1024;
  const esc = s => String(s ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[c]);
  const icon = name => HeyBossUI.icon(name);
  const size = n => n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KB` : `${(n / (1024 * 1024)).toFixed(1)} MB`;
  const api = (context, operation, requestID) => HeyBossArtifacts.rpc(context, {action:"attachment", operation}, ["list","download"].includes(operation.command), requestID);
  function mount(root, context, editor = null) {
    if (!root) return;
    let entries = [], busy = false, pending = null, queue = [], generation = 0;
    root.classList.add("file-attachments");
    root.innerHTML = `<div class="attachment-heading"><h2 hidden>Attachments</h2><span class="attachment-count"></span>${context.readonly ? "" : `<button class="icon-button attachment-choose" type="button" data-choose aria-label="Attach files" title="Attach files or drop them on the input · up to 10 MB each">${icon("paperclip")}</button>`}</div><ul class="attachment-list" aria-label="Attached files"></ul>${context.readonly ? "" : '<input type="file" multiple hidden aria-label="Attach files">'}<p class="attachment-status" role="status" aria-live="polite">Loading attachments…</p><div class="attachment-error" role="alert" hidden></div>`;
    const $ = selector => root.querySelector(selector);
    const choose = $("[data-choose]"), drop = editor || root;
    if (choose && editor) editor.append(choose);
    if (choose && !editor) choose.title = "Attach files · up to 10 MB each";
    const visibility = () => {
      $(".attachment-heading").hidden = !entries.length && !root.contains(choose);
      root.hidden = !entries.length && !root.contains(choose) && !$(".attachment-status").textContent && $(".attachment-error").hidden;
    };
    const status = message => { if (root.isConnected) { $(".attachment-status").textContent = message; visibility(); } };
    const error = (message, retry) => {
      if (!root.isConnected) return;
      const box = $(".attachment-error");box.hidden = false;
      box.innerHTML = `<span>${esc(message)}</span>${retry ? '<button class="button small" type="button">Retry</button>' : ""}`;
      visibility();
      if (retry) box.querySelector("button").onclick = () => {box.hidden = true;retry();};
    };
    const lock = value => {
      busy = value;
      root.dataset.busy = String(value);
      root.querySelectorAll("button").forEach(button => button.disabled = value);
      $("input") && ($("input").disabled = value);
      if (choose) choose.disabled = value;
      drop.classList.remove("attachment-drag-over");
    };
    const render = () => {
      if (!root.isConnected) return;
      $(".attachment-count").textContent = entries.length ? String(entries.length) : "";
      $(".attachment-heading h2").hidden = !entries.length;
      $(".attachment-list").innerHTML = entries.map(file => `<li data-file="${esc(file.id)}"><span class="attachment-file-icon" aria-hidden="true">${icon("docs")}</span><div class="attachment-info"><button class="attachment-name" type="button" data-download="${esc(file.id)}" title="Download ${esc(file.name)}">${esc(file.name)}</button><span>${size(file.size)}</span></div><button class="icon-button" type="button" data-download="${esc(file.id)}" aria-label="Download ${esc(file.name)}" title="Download">${icon("download")}</button>${context.readonly ? "" : `<button class="icon-button attachment-remove" type="button" data-remove="${esc(file.id)}" aria-label="Remove ${esc(file.name)}" title="Remove">${icon("x")}</button>`}</li>`).join("");
      if (busy) root.querySelectorAll("button").forEach(button => button.disabled = true);
      visibility();
    };
    async function refresh() {
      const ticket = ++generation;
      try {const value = await api(context, {command:"list",target:context.target});if(ticket !== generation || !root.isConnected)return;entries=value.attachments;render();status("");}
      catch(e) {status("");error(e.message, refresh);}
    }
    async function uploads() {
      if (busy) return;
      lock(true);$(".attachment-error").hidden = true;let completed = 0;
      try {
        while (pending || queue.length) {
          if (!pending) {
            const file = queue.shift();
            if (file.size > limit) {error(`${file.name} exceeds 10 MB. Choose a smaller file.`);continue;}
            status(`Uploading ${file.name}…`);
            const data = await new Promise((resolve,reject) => {
              const reader = new FileReader();reader.onload = () => resolve(reader.result.slice(reader.result.indexOf(",")+1));reader.onerror = () => reject(Error(`Could not read ${file.name}. Choose it again.`));reader.readAsDataURL(file);
            });
            pending = {operation:{command:"upload",target:context.target,name:file.name,data},id:HeyBossUI.requestId()};
          }
          status(`Uploading ${pending.operation.name}…`);
          const value = await api(context,pending.operation,pending.id);
          entries.push(value.attachment);completed++;pending = null;render();
          // A page navigation must not start new uploads to an old resource.
          if (!root.isConnected) {queue=[];break;}
        }
        status(completed ? `Attached ${completed} file${completed === 1 ? "" : "s"}.` : "");
      } catch(e) {
        status("");
        if (!e.uncertain) pending = null;
        error(e.message, pending || queue.length ? uploads : null);
      } finally {lock(false);}
    }
    if (!context.readonly) {
      const input = $("input");
      choose.onclick = () => input.click();
      input.onchange = () => {queue.push(...input.files);input.value="";uploads();};
      let depth=0;
      const files = e => e.dataTransfer?.types.includes("Files");
      drop.addEventListener("dragenter", e => {if(!files(e))return;e.preventDefault();depth++;if(!busy)drop.classList.add("attachment-drag-over");});
      drop.addEventListener("dragover", e => {if(!files(e))return;e.preventDefault();e.dataTransfer.dropEffect=busy?"none":"copy";});
      drop.addEventListener("dragleave", e => {if(!files(e))return;if(--depth<=0){depth=0;drop.classList.remove("attachment-drag-over");}});
      drop.addEventListener("drop", e => {if(!files(e))return;e.preventDefault();depth=0;drop.classList.remove("attachment-drag-over");if(busy)return;queue.push(...e.dataTransfer.files);uploads();});
    }
    root.addEventListener("click", async event => {
      const download = event.target.closest("[data-download]");
      if (download && !busy) {
        lock(true);status("Preparing download…");
        try {
          const value = await api(context,{command:"download",id:download.dataset.download});
          const binary = atob(value.data),bytes = new Uint8Array(binary.length);
          for(let i=0;i<binary.length;i++)bytes[i]=binary.charCodeAt(i);
          const url = URL.createObjectURL(new Blob([bytes],{type:"application/octet-stream"}));
          const link = document.createElement("a");link.href=url;link.download=value.attachment.name;link.click();setTimeout(()=>URL.revokeObjectURL(url),1000);status("Download ready.");
        } catch(e) {status("");error(e.message);} finally {lock(false);}
      }
      const remove = event.target.closest("[data-remove]");
      if (remove && !busy) {
        const row = remove.closest("li"),name = entries.find(f=>f.id===remove.dataset.remove).name;
        row.innerHTML = `<div class="attachment-confirm"><span>Remove ${esc(name)}?</span><div><button class="button small danger" type="button" data-confirm-remove="${esc(remove.dataset.remove)}">Remove</button><button class="button small" type="button" data-cancel-remove>Keep file</button></div></div>`;
        row.querySelector("[data-cancel-remove]").focus();
      }
      if (event.target.closest("[data-cancel-remove]")) {render();choose?.focus();}
      const confirm = event.target.closest("[data-confirm-remove]");
      if (confirm && !busy) {
        const id = confirm.dataset.confirmRemove,requestID = confirm.dataset.requestId ||= HeyBossUI.requestId();
        lock(true);status("Removing file…");
        try {await api(context,{command:"remove",id},requestID);entries=entries.filter(f=>f.id!==id);render();status("File removed.");}
        catch(e) {status("");error(e.message);}finally{lock(false);choose?.focus();}
      }
    });
    refresh();
  }
  // Hosted Markdown uses the same authenticated RPC as ordinary attachments,
  // including paired phones and remote project stores. Never fetch local paths.
  function hydrate(root,context,files=[]) {
    const identifier = value => /^\/attachments\/(f-[a-f0-9]{32})(?:[?#].*)?$/.exec(value||"")?.[1];
    const bytes = value => Uint8Array.from(atob(value.data),c=>c.charCodeAt(0));
    const imageType = data => {
      if(data[0]===137&&data[1]===80&&data[2]===78&&data[3]===71)return "image/png";
      if(data[0]===255&&data[1]===216&&data[2]===255)return "image/jpeg";
      if(String.fromCharCode(...data.slice(0,6)).match(/^GIF8[79]a$/))return "image/gif";
      if(String.fromCharCode(...data.slice(0,4))==="RIFF"&&String.fromCharCode(...data.slice(8,12))==="WEBP")return "image/webp";
      if(/^\s*(?:<\?xml[^>]*>\s*)?<svg[\s>]/i.test(new TextDecoder().decode(data.slice(0,1024))))return "image/svg+xml";
      return null;
    };
    const queue=[...root.querySelectorAll("img[data-attachment-src],img[data-import-src]")];
    const pending=new Map();
    const read=id=>{if(!pending.has(id))pending.set(id,api(context,{command:"download",id}));return pending.get(id);};
    async function load(img) {
      const id=identifier(img.dataset.attachmentSrc),local=files.find(file=>file.destination===img.dataset.importSrc);if(!id&&!local)return;
      img.setAttribute("aria-busy","true");
      try {
        const value=local||await read(id),data=bytes(value),type=imageType(data);
        if(!type)throw Error("This attachment is available as a download.");
        if(!img.isConnected)return;
        const url=URL.createObjectURL(new Blob([data],{type}));
        try{await new Promise((resolve,reject)=>{img.onload=resolve;img.onerror=()=>reject(Error("The image could not be decoded."));img.src=url;});}
        finally{URL.revokeObjectURL(url);img.onload=img.onerror=null;img.removeAttribute("aria-busy");}
      } catch(e) {
        pending.delete(id);img.removeAttribute("aria-busy");
        if(!img.isConnected)return;
        const retry=document.createElement("button");retry.type="button";retry.className="button attachment-image-retry";
        retry.textContent=(img.alt||"Image")+" — retry loading";retry.title=e.message;
        img.hidden=true;img.after(retry);retry.onclick=()=>{retry.remove();img.hidden=false;load(img);};
      }
    }
    // Keep large illustrated documents from flooding the companion tunnel.
    async function worker(){while(queue.length&&root.isConnected)await load(queue.shift());}
    Promise.all(Array.from({length:Math.min(3,queue.length)},worker)).finally(()=>pending.clear());
    root.addEventListener("click",async event=>{
      const link=event.target.closest("a[href]");if(!link||!root.contains(link))return;
      const id=identifier(link.getAttribute("href"));if(!id)return;
      event.preventDefault();if(link.getAttribute("aria-busy")==="true")return;
      link.setAttribute("aria-busy","true");
      let note=link.nextElementSibling;
      if(!note?.classList.contains("attachment-inline-status")){note=document.createElement("span");note.className="attachment-inline-status";note.setAttribute("role","status");link.after(note);}
      note.textContent=" Preparing download…";
      try {
        const value=await api(context,{command:"download",id});
        const url=URL.createObjectURL(new Blob([bytes(value)],{type:"application/octet-stream"}));
        const download=document.createElement("a");download.href=url;download.download=value.attachment.name;download.click();
        setTimeout(()=>URL.revokeObjectURL(url),1000);note.textContent=" Download ready.";
      }catch(e){note.textContent=" "+e.message;}finally{link.removeAttribute("aria-busy");}
    });
  }
  return {mount,hydrate};
})();
