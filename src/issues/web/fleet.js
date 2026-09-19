"use strict";
(() => {
  const $ = (id) => document.getElementById(id);
  HeyBossUI.icons();
  const picker = new HeyBossUI.ProjectPicker({
    onSelect(project) { location.hash = new URLSearchParams({project}).toString(); },
  });
  let projects = [], defaultProject;
  function projectContext() {
    const id = HeyBossUI.projectId(defaultProject.id);
    picker.update(projects, projects.find(p => p.id === id) || defaultProject);
  }
  addEventListener("hashchange", projectContext);
  const connection = (connected) => {
    const status = $("connection");
    status.classList.toggle("offline", !connected);
    status.querySelector("span").textContent = connected ? "Connected" : "Reconnecting…";
  };
  let csrf, refreshing = false, last = null;
  const element = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text !== undefined) e.textContent = text; return e; };
  const age = (at) => { if (!at) return "Not yet"; const s = Math.max(0, Math.floor(Date.now()/1000-at)); return s < 5 ? "Just now" : s < 60 ? `${s}s ago` : s < 3600 ? `${Math.floor(s/60)}m ago` : `${Math.floor(s/3600)}h ago`; };
  const clock = (at) => new Date(at * 1000).toLocaleTimeString([], {hour:"2-digit", minute:"2-digit", second:"2-digit"});
  const pair = (parent, label, value) => { const p = element("span", "", label + " "); p.append(element("b", "", value)); parent.append(p); };
  function render(data) {
    last = data;
    const focus = document.activeElement?.dataset.focus;
    const machines = data.machines || [], workers = machines.flatMap(m => m.workers || []);
    const busy = machines.filter(m=>m.state==="connected").flatMap(m=>m.workers||[]).reduce((n,w) => n+(w.active||0),0), slots = workers.reduce((n,w) => n+(w.config?.concurrency||0),0);
    $("overview").replaceChildren(...[[machines.filter(m=>m.state==="connected").length+" / "+machines.length,"Connected machines"],[busy+" / "+slots,"Active agents / capacity"],[machines.reduce((n,m)=>n+(m.pending||0),0),"Changes waiting to sync"],[(data.conflicts||[]).length,"Changes needing review"]].map(([value,label])=>{const c=element("div","metric glass");c.append(element("strong","",String(value)),element("span","",label));return c;}));
    const cards = machines.map(m => {
      const card = element("article","machine glass"), heading=element("div","machine-header"), title=element("div");
      title.append(element("h3","machine-title",m.hostname||m.host),element("p","machine-subtitle",(m.role==="supervisor"||m.role==="controller")?"Supervisor · coordinates workers and queue":`Companion · syncs this machine`));
      heading.append(title,element("span",`badge ${m.state}`,m.state==="connected"?"Connected":m.state==="connecting"?"Connecting":"Disconnected"));card.append(heading);
      const meta=element("div","machine-meta");pair(meta,"Heartbeat",age(m.heartbeat));pair(meta,"Last sync",(m.role==="supervisor"||m.role==="controller")?"Local":age(m.last_sync));pair(meta,"Configuration",(m.role==="supervisor"||m.role==="controller")?"Authority":m.applied_revision===m.desired_revision?"Up to date":"Applying…");pair(meta,"Software",m.deployment||m.build?.match(/build ([a-f0-9]+)/)?.[1]||"Installed");card.append(meta);
      if(m.state!=="connected")card.append(element("p","machine-notice","Workers keep running from saved configuration. Signals and changes synchronize on reconnect."));
      if(m.error||m.deployment_error||m.configuration_error)card.append(element("p","machine-notice",m.configuration_error||m.deployment_error||m.error));
      for(const w of m.workers||[]) {
        const block=element("section","worker"), h=element("div","worker-heading");h.append(element("h3","",w.config?.name||w.id),element("span","capacity",`${w.active||0} busy / ${w.config?.concurrency||1} · ${w.upgrading?"Updating":w.config?.enabled?"Pickup on":"Paused"}`));block.append(h);
        const bar=element("div","bar"), fill=element("div");fill.style.width=Math.min(100,(w.active||0)/Math.max(1,w.config?.concurrency||1)*100)+"%";bar.append(fill);block.append(bar,element("p","worker-project",(w.config?.projects||[]).map(p=>p.split("/").slice(-1)[0]).join(" · ")));
        const runs=w.runs||[], active=runs.filter(r=>r.finished_at===null);
        for(const r of active){const row=element("div","run");row.append(element("strong","",`${r.project_name||""} #${r.number} · ${r.title}`),element("small",m.state!=="connected"?"offline-run":"",m.state!=="connected"?`Last seen ${r.state} · ${age(m.heartbeat)}`:`${r.state} · ${r.last_event||"Starting…"}`));block.append(row);}
        if(!active.length)block.append(element("p","empty",`${w.eligible||0} eligible · No active agents`));
        const controls=element("div","controls");for(const [action,label] of [[w.config?.enabled?"pause":"resume",w.config?.enabled?"Pause":"Resume"],["restart","Restart"],["stop","Stop"]]){const b=element("button",action==="stop"?"button small danger":"button small",label);b.type="button";Object.assign(b.dataset,{host:m.host,worker:w.id,signal:action,focus:`${m.host}:${w.id}:${action}`});controls.append(b);}block.append(controls);
        const pending=(data.signals||[]).filter(s=>s.host===m.host&&s.worker===w.id&&s.state!=="acknowledged").slice(0,2);for(const s of pending)block.append(element("p","signal-state",`${s.signal} · ${s.state}${s.state==="pending"&&m.state!=="connected"?" until reconnect":""}`));
        const history=runs.filter(r=>r.finished_at!==null).slice(0,3);if(history.length){const details=element("details","history");details.append(element("summary","","Recent attempts · do not use slots"));for(const r of history)details.append(element("p","",`#${r.number} · ${r.state}`));block.append(details);}card.append(block);
      }
      if(!(m.workers||[]).length)card.append(element("p","worker empty","No workers configured on this machine yet."));return card;
    });
    $("machines").replaceChildren(...cards);if(focus){const b=[...document.querySelectorAll("button[data-focus]")].find(b=>b.dataset.focus===focus);b?.focus({preventScroll:true});}
    const events=(data.events||[]).filter(e=>e.kind!=="heartbeat").slice(-15).reverse();$("event-count").textContent="Live events";$("events").replaceChildren(...events.map(e=>{const li=element("li","");li.append(element("time","",clock(e.at)),element("span","event-host",e.host),element("span","event-detail",e.detail));return li;}));
    $("conflicts").hidden=!(data.conflicts||[]).length;$("conflict-list").replaceChildren(...(data.conflicts||[]).map(c=>{const li=element("li",""),details=element("details","");details.append(element("summary","",`${c.table_name}: ${c.reason} · ${c.id}`));if(c.saved_change){const pre=element("pre","",c.saved_change);details.append(pre);}li.append(details);return li;}));
  }
  async function refresh() { if(refreshing)return; refreshing=true;try{const response=await fetch("/api/fleet/status",{cache:"no-store"});const data=await response.json();if(!response.ok||data.ok===false)throw new Error(data.error?.message||data.error||"Supervisor unavailable");render(data);$("error").hidden=true;}catch(e){$("error").textContent=e.message;$("error").hidden=false;if(last){const stale=structuredClone(last);for(const m of stale.machines||[])if(Date.now()/1000-(m.heartbeat||0)>15)m.state="disconnected";render(stale);}}finally{refreshing=false;} }
  async function post(value){for(let attempt=0;attempt<2;attempt++){const response=await fetch("/api/fleet",{method:"POST",headers:{"Content-Type":"application/json","X-Hey-Boss-CSRF":csrf},body:JSON.stringify(value)});if(response.status===403&&attempt===0){const bootstrap=await fetch("/api/bootstrap",{cache:"no-store"});csrf=(await bootstrap.json()).csrf;continue;}const data=await response.json();if(!response.ok||data.ok===false)throw new Error(data.error?.message||data.error||"Signal failed");return data;}}
  $("refresh").addEventListener("click",refresh);
  $("machines").addEventListener("click",async e=>{const b=e.target.closest("button[data-signal]");if(!b)return;b.disabled=true;try{await post({kind:"signal",host:b.dataset.host,worker:b.dataset.worker,signal:b.dataset.signal,id:crypto.randomUUID()});await refresh();}catch(e){$("error").textContent=e.message;$("error").hidden=false;}finally{b.disabled=false;}});
  (async()=>{try{const response=await fetch("/api/bootstrap");const data=await response.json();csrf=data.csrf;projects=data.projects || [];defaultProject=data.project;projectContext();await refresh();const events=new EventSource("/api/fleet/events");const connected=()=>{connection(true);refresh();};events.addEventListener("connected",connected);events.onmessage=connected;events.onerror=()=>{connection(false);};}catch(e){$("error").textContent=e.message;$("error").hidden=false;}})();
  setInterval(()=>{if(!document.hidden)refresh();},15000);
  document.addEventListener("visibilitychange",()=>{if(!document.hidden)refresh();});
})();
