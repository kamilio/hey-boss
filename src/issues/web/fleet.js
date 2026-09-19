"use strict";
// Runtime membership comes from the process snapshot, never saved pickup settings.
function fleetView(data, now = Date.now()) {
  const live = [], saved = [], offline = [];
  const machines = data.machines || [];
  for (const machine of machines) {
    if (machine.state !== "connected" || now / 1000 - (machine.heartbeat || 0) > 15) {
      offline.push(machine);
      continue;
    }
    for (const worker of machine.workers || []) {
      (worker.pid > 0 ? live : saved).push({machine, worker});
    }
  }
  return {live, saved, offline,
    supervisor: machines.find(m => ["supervisor", "controller"].includes(m.role)),
    active: live.reduce((n, {worker}) => n + (worker.runs || []).filter(r => r.finished_at == null).length, 0),
    capacity: live.reduce((n, {worker}) => n + (worker.config?.concurrency || 1), 0)};
}
function elapsed(run, now = Date.now()) {
  const seconds = Math.max(0, Math.floor(((run.finished_at ?? now) - (run.started_at ?? now)) / 1000));
  const hours = Math.floor(seconds / 3600), minutes = Math.floor(seconds / 60) % 60;
  return `${hours ? hours + "h" : ""}${hours ? String(minutes).padStart(2, "0") : minutes}m${String(seconds % 60).padStart(2, "0")}s`;
}
if (typeof module !== "undefined") module.exports = {fleetView, elapsed};
if (typeof document !== "undefined") (() => {
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
  function runRow(run, machine, offline = false) {
    const row = element("div", "run"), heading = element("div", "run-heading");
    heading.append(element("strong", "", `${run.project_name || ""} #${run.number} · ${run.title || ""}`));
    const duration = element("time", "duration", elapsed(run, offline ? (machine.heartbeat || 0) * 1000 : Date.now()));
    Object.assign(duration.dataset, {started: run.started_at ?? "", finished: run.finished_at ?? "", snapshot: offline ? (machine.heartbeat || 0) * 1000 : ""});
    duration.setAttribute("aria-label", `Time on task: ${duration.textContent}`);
    heading.append(duration); row.append(heading);
    const meta = element("div", "run-meta");
    meta.append(element("span", offline ? "offline-run" : "", `${offline ? "Last seen " : ""}${run.state} · Codex${run.session_id ? "" : " · not launched"}`));
    if (run.goal?.status) meta.append(element("span", "", `Goal: ${run.goal.status}`));
    if (run.reservation_expires && run.finished_at == null && !offline) meta.append(element("span", "", `Claim in ${Math.max(0, Math.ceil((run.reservation_expires - Date.now()) / 1000))}s`));
    if (run.session_id) {
      const copy = element("button", "button small copy-session", "Copy session ID");
      copy.type = "button";
      Object.assign(copy.dataset, {session: run.session_id, focus: `${machine.host}:${run.id}:copy`});
      copy.setAttribute("aria-label", `Copy Codex session ID for ${run.project_name || "issue"} #${run.number}`);
      meta.append(copy);
    }
    row.append(meta);
    if (run.last_event || run.summary) row.append(element("small", "run-event", run.last_event || run.summary));
    return row;
  }
  function workerBlock(machine, worker, data, mode = "live") {
    const block = element("section", "worker"), heading = element("div", "worker-heading");
    const active = (worker.runs || []).filter(r => r.finished_at == null);
    const status = mode === "offline" ? "Last known worker" : mode === "saved" ? "Not running" : worker.upgrading ? "Updating · draining" : worker.config?.enabled ? "Running" : active.length ? "Paused · draining" : "Paused";
    heading.append(element("h3", "", worker.config?.name || "Worker"), element("span", "capacity", `${status}${mode === "live" ? ` · ${active.length} / ${worker.config?.concurrency || 1} agents` : ""}`));
    block.append(heading, element("p", "worker-project", `${machine.hostname || machine.host} · ${(worker.config?.projects || []).map(p => p.split("/").slice(-1)[0]).join(" · ") || "All projects"}`));
    const agents = element("div", "agents");
    for (const run of active) agents.append(runRow(run, machine, mode !== "live"));
    if (!active.length) agents.append(element("p", "empty", mode === "live" ? `No active agents · ${worker.eligible || 0} eligible tasks` : "No active agents reported"));
    block.append(agents);
    const controls = element("div", "controls");
    const toggle = mode === "saved" || !worker.config?.enabled ? ["resume", "Resume"] : ["pause", "Pause"];
    for (const [action, label] of [toggle, ["restart", "Restart"], ["stop", "Stop"]]) {
      const button = element("button", action === "stop" ? "button small danger" : "button small", label);
      button.type = "button";
      Object.assign(button.dataset, {host: machine.host, worker: worker.id, signal: action, focus: `${machine.host}:${worker.id}:${action}`});
      button.setAttribute("aria-label", `${label} ${worker.config?.name || "worker"} on ${machine.hostname || machine.host}`);
      controls.append(button);
    }
    block.append(controls);
    const pending = (data.signals || []).filter(s => s.host === machine.host && s.worker === worker.id && s.state !== "acknowledged").slice(0, 2);
    for (const signal of pending) block.append(element("p", "signal-state", `${signal.signal} · ${signal.state}${mode === "offline" ? " · queued until reconnect" : ""}`));
    const history = (worker.runs || []).filter(r => r.finished_at != null);
    if (history.length) {
      const details = element("details", "history");
      details.dataset.section = `history:${machine.host}:${worker.id}`;
      details.append(element("summary", "", `Recent attempts (${history.length})`));
      for (const run of history) details.append(runRow(run, machine));
      block.append(details);
    }
    return block;
  }
  function render(data) {
    last = data;
    const focus = document.activeElement?.dataset.focus;
    const open = new Set([...$("machines").querySelectorAll("details[open]")].map(d => d.dataset.section));
    const view = fleetView(data), machines = data.machines || [];
    $("overview").replaceChildren(...[[view.live.length, "Running workers"], [view.active, "Active agents"], [view.capacity, "Agent capacity"], [view.offline.length, "Disconnected machines"]].map(([value, label]) => {
      const card = element("div", "metric glass"); card.append(element("strong", "", String(value)), element("span", "", label)); return card;
    }));
    const tree = element("article", "supervisor glass"), heading = element("div", "machine-header"), title = element("div");
    title.append(element("h2", "machine-title", "Supervisor"), element("p", "machine-subtitle", `${view.supervisor?.hostname || view.supervisor?.host || "Fleet"} · coordinates workers and queue`));
    heading.append(title); tree.append(heading);
    const workers = element("div", "worker-tree");
    for (const {machine, worker} of view.live) workers.append(workerBlock(machine, worker, data));
    if (!view.live.length) workers.append(element("p", "empty", "No workers running right now."));
    tree.append(workers);
    const sections = [tree];
    if (view.saved.length) {
      const saved = element("details", "secondary glass"); saved.dataset.section = "saved";
      saved.append(element("summary", "", `Saved workers · not running (${view.saved.length})`));
      for (const {machine, worker} of view.saved) saved.append(workerBlock(machine, worker, data, "saved"));
      sections.push(saved);
    }
    if (view.offline.length) {
      const offline = element("details", "secondary glass"); offline.dataset.section = "offline";
      offline.append(element("summary", "", `Disconnected machines · last known activity (${view.offline.length})`));
      offline.append(element("p", "machine-notice", "These snapshots are not included in running counts. Work may continue while disconnected."));
      for (const machine of view.offline) {
        const card = element("section", "offline-machine");
        card.append(element("h3", "machine-title", machine.hostname || machine.host), element("p", "machine-subtitle", `Last seen ${age(machine.heartbeat)}`));
        for (const worker of machine.workers || []) card.append(workerBlock(machine, worker, data, "offline"));
        offline.append(card);
      }
      sections.push(offline);
    }
    const diagnostics = element("details", "secondary glass"); diagnostics.dataset.section = "diagnostics";
    diagnostics.append(element("summary", "", "Machine connections and sync"));
    for (const machine of machines) {
      const card = element("section", "diagnostic-machine");
      card.append(element("h3", "machine-title", machine.hostname || machine.host));
      const meta = element("div", "machine-meta");
      pair(meta, "Connection", view.offline.includes(machine) ? "Disconnected" : "Connected");
      pair(meta, "Heartbeat", age(machine.heartbeat)); pair(meta, "Waiting to sync", machine.pending || 0);
      pair(meta, "Software", machine.deployment || machine.build?.match(/build ([a-f0-9]+)/)?.[1] || "Installed"); card.append(meta);
      if (machine.error || machine.deployment_error || machine.configuration_error) card.append(element("p", "machine-notice", machine.configuration_error || machine.deployment_error || machine.error));
      diagnostics.append(card);
    }
    sections.push(diagnostics);
    $("machines").replaceChildren(...sections);
    for (const details of $("machines").querySelectorAll("details")) details.open = open.has(details.dataset.section);
    if (focus) [...document.querySelectorAll("button[data-focus]")].find(b => b.dataset.focus === focus)?.focus({preventScroll: true});
    const events=(data.events||[]).filter(e=>e.kind!=="heartbeat").slice(-15).reverse();$("event-count").textContent="Live events";$("events").replaceChildren(...events.map(e=>{const li=element("li","");li.append(element("time","",clock(e.at)),element("span","event-host",e.host),element("span","event-detail",e.detail));return li;}));
    $("conflicts").hidden=!(data.conflicts||[]).length;$("conflict-list").replaceChildren(...(data.conflicts||[]).map(c=>{const li=element("li",""),details=element("details","");details.append(element("summary","",`${c.table_name}: ${c.reason} · ${c.id}`));if(c.saved_change){const pre=element("pre","",c.saved_change);details.append(pre);}li.append(details);return li;}));
  }
  async function refresh() { if(refreshing)return; refreshing=true;try{const response=await fetch("/api/fleet/status",{cache:"no-store"});const data=await response.json();if(!response.ok||data.ok===false)throw new Error(data.error?.message||data.error||"Supervisor unavailable");render(data);$("error").hidden=true;}catch(e){$("error").textContent=e.message;$("error").hidden=false;if(last){const stale=structuredClone(last);for(const m of stale.machines||[])if(Date.now()/1000-(m.heartbeat||0)>15)m.state="disconnected";render(stale);}}finally{refreshing=false;} }
  async function post(value){for(let attempt=0;attempt<2;attempt++){const response=await fetch("/api/fleet",{method:"POST",headers:{"Content-Type":"application/json","X-Hey-Boss-CSRF":csrf},body:JSON.stringify(value)});if(response.status===403&&attempt===0){const bootstrap=await fetch("/api/bootstrap",{cache:"no-store"});csrf=(await bootstrap.json()).csrf;continue;}const data=await response.json();if(!response.ok||data.ok===false)throw new Error(data.error?.message||data.error||"Signal failed");return data;}}
  $("refresh").addEventListener("click",refresh);
  $("machines").addEventListener("click", async event => {
    const button = event.target.closest("button[data-session]");
    if (!button) return;
    try {
      if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(button.dataset.session);
      else {
        const input = element("textarea", "clipboard-input", button.dataset.session);
        document.body.append(input); input.select();
        let copied;
        try { copied = document.execCommand("copy"); } finally { input.remove(); button.focus({preventScroll: true}); }
        if (!copied) throw new Error("Clipboard unavailable. Copy session ID from a secure browser connection.");
      }
      $("copy-status").textContent = "Session ID copied.";
    } catch (error) { $("error").textContent = error.message; $("error").hidden = false; }
  });
  $("machines").addEventListener("click",async e=>{const b=e.target.closest("button[data-signal]");if(!b)return;b.disabled=true;try{await post({kind:"signal",host:b.dataset.host,worker:b.dataset.worker,signal:b.dataset.signal,id:crypto.randomUUID()});await refresh();}catch(e){$("error").textContent=e.message;$("error").hidden=false;}finally{b.disabled=false;}});
  (async()=>{try{const response=await fetch("/api/bootstrap");const data=await response.json();csrf=data.csrf;projects=data.projects || [];defaultProject=data.project;projectContext();await refresh();const events=new EventSource("/api/fleet/events");const connected=()=>{connection(true);refresh();};events.addEventListener("connected",connected);events.onmessage=connected;events.onerror=()=>{connection(false);};}catch(e){$("error").textContent=e.message;$("error").hidden=false;}})();
  setInterval(() => {
    if (document.hidden) return;
    for (const time of document.querySelectorAll('time.duration[data-finished=""][data-snapshot=""]')) {
      time.textContent = elapsed({started_at: time.dataset.started === "" ? undefined : Number(time.dataset.started)}, Date.now());
      time.setAttribute("aria-label", `Time on task: ${time.textContent}`);
    }
  }, 1000);
  setInterval(()=>{if(!document.hidden)refresh();},15000);
  document.addEventListener("visibilitychange",()=>{if(!document.hidden)refresh();});
})();
