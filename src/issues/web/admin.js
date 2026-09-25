"use strict";
(() => {
  const $ = s => document.querySelector(s);
  const esc = s => String(s ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
  let data, csrf, projects = [], issue = null, selection = "prompt:worker", sequence = 0, contextSequence = 0;
  const cache = new Map();
  const counts = text => { const lines=text.split("\n").length; return `${text.length.toLocaleString()} characters · ${lines} ${lines===1?"line":"lines"}`; };
  function fail(error) { $("#error").textContent = error.message || String(error); $("#error").hidden = false; }
  async function request(path, body) {
    const response = await fetch(path, {method:body ? "POST" : "GET", headers:body ? {"Content-Type":"application/json","X-Hey-Boss-CSRF":csrf} : {}, body:body ? JSON.stringify(body) : undefined, signal:AbortSignal.timeout(40000)});
    const value = await response.json();
    if (!response.ok || value.ok === false) throw new Error(value.error?.message || value.error || `Request failed (${response.status})`);
    return value;
  }
  const api = operation => request("/api/action", {project:$("#project").value, operation});
  function nav() {
    const query = $("#search").value.toLowerCase().trim();
    const matches = (name, text="") => `${name} ${text}`.toLowerCase().includes(query);
    const button = (id,title,count,cmd=false) => `<button type="button" data-select="${esc(id)}" aria-current="${selection===id}"><span class="${cmd?"cmd-name":""}">${esc(title)}</span><span class="count">${esc(count)}</span></button>`;
    let html = '<h2>PROMPTS <span>7</span></h2>';
    data.prompts.filter(p=>matches(p.title,p.text)).forEach(p=>html+=button(`prompt:${p.id}`,p.title,`${p.text.length.toLocaleString()} ch`));
    if(matches("Complete agent input")) html+=button("assembled","Complete agent input","LIVE");
    html+='<h2>AGENT GUIDANCE</h2>';
    if(matches("Skill",data.skill.text)) html+=button("skill","Hey Boss skill",`${data.skill.text.length.toLocaleString()} ch`);
    (data.skill.references||[]).filter(r=>matches(r.title,r.text)).forEach(r=>html+=button(`reference:${r.path}`,r.title,"REFERENCE"));
    if(matches("Page guide",data.guide.text)) html+=button("guide","Page discovery guide","WEB");
    const commands=data.commands.filter(c=>matches(c.id,c.description));
    html+=`<h2>COMMANDS <span>${commands.length} / ${data.commands.length}</span></h2>`;
    let group="";
    for(const c of commands) { if(c.group!==group){group=c.group;html+=`<h2>${esc(group)}</h2>`;} html+=button(`command:${c.id}`,c.id,c.preview==="sample"?"OUTPUT":"HELP",true); }
    $("#catalog").innerHTML=html;
  }
  function heading(kind,title,description,extra="") {
    return `<div class="page-heading"><div><span class="eyebrow">${esc(kind)}</span><h2>${esc(title)}</h2><p>${esc(description)}</p></div>${extra}</div>`;
  }
  function panel(title,text,meta="",id="source") {
    return `<section class="panel"><div class="panel-head"><span>${esc(title)}</span><span class="meta">${esc(meta)}</span><button type="button" class="button" data-copy="${esc(id)}">Copy</button></div><pre id="${esc(id)}">${esc(text)}</pre></section>`;
  }
  function source(kind,title,text,path,extra="") {
    $("#content").innerHTML=heading(kind,title,"Review the exact Markdown included in this build.")+`<div class="stats"><span class="pill">${counts(text)}</span><span class="pill source">${esc(path)}</span></div>${extra}${panel("Source",text)}`;
  }
  async function show(id) {
    selection=id; const run=++sequence; nav(); $("#error").hidden=true;
    history.replaceState(null,"",`#${new URLSearchParams({item:id,project:$("#project").value,issue:$("#issue").value})}`);
    if(id.startsWith("prompt:")) {
      const p=data.prompts.find(p=>p.id===id.slice(7));
      if(!p) return show("prompt:worker");
      source("PROMPT TEMPLATE",p.title,p.text,p.source,'<div class="toolbar"><button class="button" data-select="assembled">See complete agent input →</button><span class="source">Project overrides can replace these defaults.</span></div>');
    } else if(id==="skill") {
      const references=(data.skill.references||[]).map(r=>`<button class="button" data-select="reference:${esc(r.path)}">${esc(r.title)} →</button>`).join("");
      source("AGENT SKILL","Hey Boss skill",data.skill.text,data.skill.source,`<div class="note">Install the skill and its references into the current user’s Codex, Agents and Claude Code skill directories. Read references only when needed.</div>${panel("Install command",data.skill.install,"Run in your terminal","install-command")}<div class="toolbar">${references}</div>`);
    } else if(id.startsWith("reference:")) {
      const r=(data.skill.references||[]).find(r=>r.path===id.slice(10));
      if(!r) return show("skill");
      source("SKILL REFERENCE",r.title,r.text,r.source,'<div class="toolbar"><button class="button" data-select="skill">← Hey Boss skill</button></div>');
    } else if(id==="guide") {
      source("WEB GUIDANCE","Page discovery guide",data.guide.text,data.guide.source);
    } else if(id==="assembled") {
      $("#content").innerHTML=heading("LIVE PREVIEW","Complete agent input","The actual worker preview for the selected project and issue, including applicable project overrides and delivery instructions.")+`<div class="toolbar"><label>Workflow <select id="workflow"><option value="checkout">Implement · existing checkout · main</option><option value="worktree">Implement · worktree · PR</option><option value="plan">Plan · artifact</option><option value="chief">Chief · organizing pass</option></select></label><button class="button" id="refresh-assembled">Refresh</button></div><div id="assembled-output"><div class="empty">Loading expanded prompt…</div></div>`;
      $("#workflow").addEventListener("change",assembled);$("#refresh-assembled").addEventListener("click",assembled);
      await assembled();
    } else if(id.startsWith("command:")) {
      const c=data.commands.find(c=>c.id===id.slice(8)); if(!c){const grouped=data.commands.find(c=>c.id===`notif ${id.slice(8)}`);return show(grouped?`command:${grouped.id}`:"prompt:worker");}
      $("#content").innerHTML=heading("COMMAND OUTPUT",`hey-boss ${c.id}`,c.description,`<button class="button" id="refresh-command">Refresh preview</button>`)+`<div class="stats"><span class="pill">${c.json_supported?"Text + JSON option":"No --json option"}</span>${c.aliases.length?`<span class="pill">Aliases: ${esc(c.aliases.join(", "))}</span>`:""}<span class="pill">${c.preview==="sample"?"Disposable sample state":"Help only · external effects"}</span></div><div id="command-output"><div class="empty">Capturing output…</div></div><details><summary>Command help & options</summary><pre>${esc(c.help)}</pre></details>`;
      $("#refresh-command").addEventListener("click",()=>{cache.delete(id);show(id).catch(fail);});
      const preview=cache.get(id)||await request("/api/admin/preview",{command:c.id,issue:issue?{title:issue.title,body:String(issue.body||"").slice(0,60000)}:{}});
      if(run!==sequence)return; cache.set(id,preview);
      if(cache.size>12)cache.delete(cache.keys().next().value);
      const output=(name,result)=>{
        if(!result)return `<section class="panel"><div class="panel-head">${name}</div><div class="empty">${!c.json_supported?"This command has no --json option.":"No JSON result is captured for this external command."}</div></section>`;
        let stdout=result.stdout||"";
        if(name==="JSON"&&stdout){try{stdout=JSON.stringify(JSON.parse(stdout),null,2)}catch{}}
        return `<section class="panel"><div class="panel-head"><span>${name}</span><span class="meta">${result.timed_out?"Timed out":`Exit ${result.exit_code}`} · ${counts(stdout)}</span><button class="button" data-copy="out-${name}">Copy</button></div><div class="invocation">${esc(result.command)}</div><pre id="out-${name}">${esc(stdout||"(no stdout)")}</pre>${result.stderr?`<pre class="stderr">${esc(result.stderr)}</pre>`:""}${result.truncated?'<div class="note">Output truncated at 1 MiB.</div>':""}</section>`;
      };
      $("#command-output").innerHTML=`<div class="note">${esc(preview.reason)}</div><div class="outputs">${output(preview.mode==="help"?"Help":"Text",preview.text)}${output("JSON",preview.json)}</div>`;
    }
  }
  async function assembled() {
    const run=++sequence;const workflow=$("#workflow")?.value||"checkout";
    if(!issue&&workflow!=="chief"){$("#assembled-output").innerHTML='<div class="empty">Choose a project with an issue to preview its full agent input.</div>';return;}
    try {
      if(workflow==="chief") {
        const settings=await api({action:"project_settings"});
        if(run!==sequence||!$("#assembled-output"))return;
        const text=data.chief_wrapper.replace(/{{(project|prompt)}}/g,(_,key)=>key==="project"?$("#project").value:settings.chief_prompt);
        $("#assembled-output").innerHTML=panel("Chief input",text,counts(text),"expanded");return;
      }
      const config={projects:[$("#project").value],prs_enabled:workflow==="worktree",worktree_enabled:workflow==="worktree"};
      const result=await api({action:"preview_worker",config,number:issue.number,task_kind:workflow==="plan"?"plan":"implement",worktree_allowed:workflow==="worktree"});
      if(run!==sequence||!$("#assembled-output"))return;
      $("#assembled-output").innerHTML=panel("Expanded prompt",result.prompt,counts(result.prompt),"expanded")+`<details><summary>Full preview JSON</summary><pre>${esc(JSON.stringify(result,null,2))}</pre></details>`;
    }catch(e){fail(e)}
  }
  async function loadIssue() {
    const run=++contextSequence;$("#context-status").textContent="Loading issue…";
    const number=Number($("#issue").value);
    const value=number?await api({action:"view",number}):null;
    if(run!==contextSequence)return;
    issue=value?.issue||null;cache.clear();
    $("#context-status").textContent=issue?`Real issue #${issue.number} · command samples use a disposable copy`:"No issues · sample command data";
    await show(selection);
  }
  async function loadProject(preferredIssue) {
    const run=++contextSequence;$("#context-status").textContent="Loading issues…";
    const value=await api({action:"list",state:"all",mine:false,unassigned:false,labels:[],search:null,limit:50,offset:0});
    if(run!==contextSequence)return;
    $("#issue").innerHTML=(value.issues||[]).map(i=>`<option value="${i.number}">#${i.number} · ${esc(i.title)}</option>`).join("")||'<option value="">No issues</option>';
    if(preferredIssue&&[...$("#issue").options].some(o=>o.value===preferredIssue))$("#issue").value=preferredIssue;
    await loadIssue();
  }
  document.addEventListener("click",async event=>{
    const selected=event.target.closest("[data-select]"); if(selected){show(selected.dataset.select).catch(fail);return;}
    const copy=event.target.closest("[data-copy]");if(!copy)return;
    const value=document.getElementById(copy.dataset.copy)?.textContent||"";
    try{if(navigator.clipboard&&window.isSecureContext)await navigator.clipboard.writeText(value);else{const area=document.createElement("textarea");area.value=value;document.body.append(area);area.select();if(!document.execCommand("copy"))throw new Error("Copy unavailable");area.remove();}$("#toast").textContent="Copied";$("#toast").hidden=false;setTimeout(()=>$("#toast").hidden=true,1800)}catch(e){fail(e)}
  });
  $("#search").addEventListener("input",()=>data&&nav());
  $("#project").addEventListener("change",()=>loadProject().catch(fail));
  $("#issue").addEventListener("change",()=>loadIssue().catch(fail));
  async function start() {
    const [catalog,boot]=await Promise.all([request("/api/admin/catalog"),request("/api/bootstrap")]);
    data=catalog;csrf=boot.csrf;projects=boot.projects||[];
    $("#build").textContent=`BUILD ${data.build} · ${data.commands.filter(c=>c.preview==="sample").length} output previews`;
    const route=new URLSearchParams(location.hash.slice(1));selection=route.get("item")||selection;
    $("#project").innerHTML=projects.map(p=>`<option value="${esc(p.id)}">${esc(p.name)}</option>`).join("");
    const preferred=route.get("project")||boot.project?.id;
    if(projects.some(p=>p.id===preferred))$("#project").value=preferred;
    nav();
    if(projects.length)await loadProject(route.get("issue"));else{$("#context-status").textContent="No projects · sample command data";await show(selection);}
  }
  start().catch(fail);
})();
