"use strict";
(() => {
  const $ = (s) => document.querySelector(s);
  const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  let csrf = "", graph = null, projects = [], generation = 0, controller = null, boot = null;
  const collapsed = new Set(), openBodies = new Set();
  let viewMode = "map", selected = null, mapQuery = "", mapSearchCamera = null;
  let detailsHTML = "", detailsNode = null;
  const fullBodies = new Map(), bodyVersions = new Map(), loadingBodies = new Map(), bodyErrors = new Map(), initializedProjects = new Set();
  const route = () => new URLSearchParams(location.hash.slice(1));
  const mapUrl = (project, node) => `/mm#${new URLSearchParams({ project, ...(node ? { node } : {}) })}`;
  const issueUrl = (node) => `/#${new URLSearchParams({ project: node.reference_project, issue: node.reference, ...(boot?.backend_host ? { host: boot.backend_host } : {}) })}`;
  const issueContext = (node) => `${node.reference_project !== graph.project.id ? `${node.reference_project_name || projects.find((p) => p.id === node.reference_project)?.name || node.reference_project} · ` : ""}issue #${node.reference}`;
  const nodeName = (node) => `${node.project_id !== graph.project.id ? `${node.project_name || projects.find((p) => p.id === node.project_id)?.name || node.project_id} · ` : ""}${node.title}`;
  const assigneeName = (id) => {
    if (!id) return "Unassigned";
    if (id === "human:boss") return graph?.boss?.name || boot?.boss?.name || "Boss";
    if (id.startsWith("codex:")) return `Codex · ${id.slice(6, 14)}`;
    if (id.startsWith("claude:")) return `Claude · ${id.slice(7, 15)}`;
    return id.replace(/^human:/, "").split("@")[0];
  };
  const allNodes = () => new Map([...graph.nodes, ...graph.external_nodes].map((node) => [node.id, fullBodies.get(node.id) || node]));
  function relationships(node, nodes, incidents) {
    return (incidents.get(node.id) || []).map((link) => {
      const outgoing = link.from === node.id, other = nodes.get(outgoing ? link.to : link.from);
      if (!other) return "";
      const text = link.kind === "depends-on" ? (outgoing ? "Depends on" : "Required by") : link.kind === "pull-request" ? (outgoing ? "Pull request" : "Issue") : `${outgoing ? "→" : "←"} ${link.kind}`;
      return `<li><span class="relation-kind">${esc(text)}</span> <a data-map-link href="${esc(other.resource_only && other.kind === "issue" ? issueUrl(other) : mapUrl(other.project_id, other.id))}">${esc(nodeName(other))}</a>${other.kind === "issue" ? ` <span class="description">(${esc(issueContext(other))})</span> ` : ""}${link.description ? `<span class="description">— ${esc(link.description)}</span>` : ""}${link.automatic ? ' <span class="automatic">automatic</span>' : ""}</li>`;
    }).join("");
  }
  function bodyContent(node) {
    const full = fullBodies.has(node.id);
    const error = bodyErrors.has(node.id) ? `<p class="body-error" role="alert">${esc(bodyErrors.get(node.id))}</p>` : "";
    return `${full ? `<button class="read-body" data-collapse-body="${esc(node.id)}" ${loadingBodies.has(node.id) ? "disabled" : ""}>Show less</button>${error}` : ""}<div class="body" id="body-${esc(node.id)}" tabindex="-1">${node.body_html || esc(node.body)}</div>${node.body_truncated ? `<button class="read-body" data-read-body="${esc(node.id)}" ${loadingBodies.has(node.id) ? "disabled" : ""}>${loadingBodies.has(node.id) ? "Loading…" : "Read full text"}</button>` : ""}${full ? "" : error}`;
  }
  const mapUI = new HeyBossMap.Mindmap($("#mindmap"), {
    select(id) { selected = id; render(); mapUI.focus(id, false); $("#map-details h2")?.focus({preventScroll:true}); },
    toggle(id) {
      const before = mapUI.data?.positions.get(id), y = before ? before.y * mapUI.camera.scale + mapUI.camera.y : null;
      collapsed.has(id) ? collapsed.delete(id) : collapsed.add(id); render();
      const after = mapUI.data?.positions.get(id); if (after && y !== null) { mapUI.camera.y = y - after.y * mapUI.camera.scale; mapUI.schedule(); }
    },
    escape() { closeInspector(); }
  });
  function closeInspector() {
    const previous = selected; selected = null; render();
    if (previous) {
      const nodes = allNodes(); let target = previous;
      while (target && !mapUI.data?.positions.has(target)) target = nodes.get(target)?.parent_id;
      if (!target || !mapUI.focus(target)) $("#mindmap").focus({preventScroll:true});
    }
  }
  function branches(expand) {
    const roots = mapUI.data?.items.filter(n => !mapUI.data.positions.has(n.parent_id)) || [];
    const center = $("#mindmap").clientHeight / 2, camera = mapUI.camera;
    const anchor = viewMode === "map" ? roots.reduce((best, n) => !best || Math.abs((n.y + 42) * camera.scale + camera.y - center) < Math.abs((best.y + 42) * camera.scale + camera.y - center) ? n : best, null) : null;
    const y = anchor ? anchor.y * camera.scale + camera.y : null;
    if (expand) collapsed.clear(); else if (graph) graph.nodes.forEach(n => collapsed.add(n.id));
    render();
    const after = anchor && mapUI.data?.positions.get(anchor.id);
    if (after && y !== null) { camera.y = y - after.y * camera.scale; mapUI.schedule(); }
  }
  function details(nodes, incidents, visible) {
    const node = selected && visible.has(selected) ? nodes.get(selected) : null;
    const container = $("#map-details");
    $("#map-inspector").hidden = !node;
    if (!node) { container.innerHTML = ""; detailsHTML = ""; detailsNode = null; return; }
    const resource = node.kind === "issue" && node.available !== false ? `<a class="map-resource-link" href="${esc(issueUrl(node))}">Open ${esc(issueContext(node))} ↗</a>` : node.kind === "pr" ? `<a class="map-resource-link" href="${esc(node.reference)}" target="_blank" rel="noopener noreferrer">Open pull request ↗</a>` : node.kind === "notification" ? `<a class="map-resource-link" href="/#${esc(new URLSearchParams({view:"inbox",notice:node.reference}).toString())}">Open notification ↗</a>` : "";
    const rel = relationships(node, nodes, incidents);
    const children = graph.nodes.filter(n => n.parent_id === node.id);
    const topics = children.length ? `<section class="topic-children" aria-label="Child topics"><h3>Topics <span>${children.length}</span></h3><ul>${children.slice(0,50).map(n => `<li><button type="button" data-select-topic="${esc(n.id)}">${esc(nodes.get(n.id).title)}</button></li>`).join("")}</ul>${children.length > 50 ? '<button type="button" data-topic-outline>View all in outline</button>' : ""}</section>` : "";
    const html = `<div id="${esc(node.id)}"><h2 tabindex="-1">${esc(node.title)}</h2><div class="meta"><span class="badge">${esc(node.kind)}</span>${node.state ? `<span>${esc(node.state)}</span>` : ""}${node.alias ? `<code>${esc(node.alias)}</code>` : ""}${node.assignee ? `<span>Assigned to ${esc(assigneeName(node.assignee))}</span>` : ""}${node.available === false ? "<span>Resource unavailable</span>" : ""}</div>${resource}${node.has_body || node.body ? bodyContent(node) : ""}${topics}${rel ? `<ul class="relationships" aria-label="Relationships for ${esc(node.title)}">${rel}</ul>` : ""}</div>`;
    if (html === detailsHTML) return;
    const same = detailsNode === node.id, active = same && container.contains(document.activeElement) ? document.activeElement : null;
    const heading = active?.matches("h2[tabindex]"), id = active?.id, read = active?.dataset.readBody, less = active?.dataset.collapseBody, topic = active?.dataset.selectTopic, outline = active?.hasAttribute("data-topic-outline"), href = active?.tagName === "A" ? active.getAttribute("href") : null;
    container.innerHTML = html; detailsHTML = html; detailsNode = node.id;
    if (!same) container.scrollTop = 0;
    if (active) {
      const target = heading ? container.querySelector("h2[tabindex]") : id ? document.getElementById(id) : read ? container.querySelector(`[data-read-body="${read}"]`) : less ? container.querySelector(`[data-collapse-body="${less}"]`) : topic ? container.querySelector(`[data-select-topic="${topic}"]`) : outline ? container.querySelector("[data-topic-outline]") : href ? [...container.querySelectorAll("a")].find(a => a.getAttribute("href") === href) : null;
      (target && !target.disabled ? target : container.querySelector("h2[tabindex]"))?.focus({preventScroll:true});
    }
  }
  async function readBody(id, mode = "full") {
    if (loadingBodies.has(id) || !graph) return;
    const ticket = generation, project = graph.project.id, signal = controller.signal, query = $("#search").value;
    loadingBodies.set(id, ticket); bodyErrors.delete(id); render();
    try {
      let value;
      for (let attempt = 0; attempt < 2; attempt++) {
        const response = await fetch("/api/mm", {method:"POST",signal,headers:{"Content-Type":"application/json","X-Hey-Boss-CSRF":csrf},body:JSON.stringify({project,operation:{action:"mindmap",operation:{command:"view",node:id,body_mode:mode}},request_id:null})});
        value = await response.json();
        if (ticket !== generation) return;
        if (response.status === 403 && attempt === 0) {
          const bootstrap = await fetch("/api/bootstrap",{signal}); const fresh = await bootstrap.json();
          if (ticket !== generation) return;
          if (!bootstrap.ok || !fresh.ok) throw new Error(fresh.error?.message || "Cannot reconnect");
          csrf = fresh.csrf; continue;
        }
        if (!response.ok || !value.ok) throw new Error(value.error?.message || "Cannot read full text");
        break;
      }
      if (ticket !== generation || project !== graph.project.id) return;
      if (!value.node) { await load({refresh:true}); return; }
      const current = graph.nodes.find((node) => node.id === id);
      if (!current) return;
      const latest = fullBodies.get(id) || current;
      if (value.node.available !== current.available) { await load({refresh:true}); return; }
      if (value.version < Math.max(graph.version, bodyVersions.get(id) || 0) || (latest.resource_version && value.node.resource_version < latest.resource_version)) throw new Error("The outline changed while reading");
      const fresh = {...value.node,parent_id:current.parent_id,position:current.position,alias:current.alias};
      if (value.boss?.version >= (graph.boss?.version || 0)) graph.boss = value.boss;
      if (mode === "preview") { graph.nodes[graph.nodes.indexOf(current)] = fresh; fullBodies.delete(id); }
      else fullBodies.set(id, fresh);
      bodyVersions.set(id, value.version);
      loadingBodies.delete(id); render();
      if (!document.getElementById(id) && query && $("#search").value === query) {
        $("#search").value = "";
        const nodes = allNodes(); let node = nodes.get(id);
        while (node) { collapsed.delete(node.id); node = nodes.get(node.parent_id); }
        render();
      }
      const focus = document.getElementById(`body-${id}`) || document.getElementById(id);
      if (focus && document.activeElement !== $("#search")) { focus.tabIndex = -1; focus.focus({preventScroll:true}); }
    } catch (error) {
      if (ticket !== generation || error.name === "AbortError") return;
      bodyErrors.set(id,`${error.message}. ${mode === "preview" ? "Try Show less again." : "Retry to load the full text."}`);
    } finally {
      if (loadingBodies.get(id) === ticket) {
        loadingBodies.delete(id); render();
        if (bodyErrors.has(id) && document.activeElement !== $("#search")) document.querySelector(`[data-${mode === "preview" ? "collapse" : "read"}-body="${id}"]`)?.focus({preventScroll:true});
      }
    }
  }
  function render() {
    if (!graph) return;
    const focus = document.activeElement, toggleFocus = focus?.dataset?.toggle;
    const nodes = allNodes(), children = new Map(), query = $("#search").value.trim().toLowerCase();
    for (const node of graph.nodes) { const key = node.parent_id || ""; if (!children.has(key)) children.set(key, []); children.get(key).push(node); }
    const incidents = new Map();
    for (const link of graph.links) {
      for (const id of [link.from, link.to]) {
        if (!incidents.has(id)) incidents.set(id, []);
        incidents.get(id).push(link);
      }
    }
    const visible = new Set(), matched = new Set();
    for (const node of graph.nodes) {
      const rel = query ? incidents.get(node.id) || [] : [];
      const display = nodes.get(node.id);
      if (!query || [display.title, display.body, display.state, display.assignee, node.kind === "issue" && display.available !== false ? assigneeName(display.assignee) : "", node.alias, node.kind, node.reference, node.reference_project, node.reference_project_name, ...rel.flatMap((l) => [l.kind, l.description, nodes.get(l.from)?.title, nodes.get(l.to)?.title])].join(" ").toLowerCase().includes(query)) {
        matched.add(node.id);
        let current = node;
        while (current && !visible.has(current.id)) { visible.add(current.id); current = nodes.get(current.parent_id); }
      }
    }
    $("#count").textContent = `${graph.nodes.length} ${graph.nodes.length === 1 ? "node" : "nodes"} · ${graph.links.length} ${graph.links.length === 1 ? "link" : "links"}`;
    $("#revision").textContent = `Map version ${graph.version}`;
    $("#outline").hidden = viewMode !== "outline"; $("#map-panel").hidden = viewMode !== "map";
    if (viewMode === "map") {
      $("#outline").innerHTML = "";
      if (mapQuery !== query && query && !mapQuery) mapSearchCamera = {...mapUI.camera};
      mapUI.update(graph.nodes.map(n => nodes.get(n.id)), {collapsed, matches:query ? visible : null, hits:query ? matched : null, project:graph.project, selected, links:graph.links, assigneeName});
      if (mapQuery !== query) {
        mapQuery = query;
        if (query) { mapUI.fit(); const first = matched.values().next().value; if (first) mapUI.focus(first, false); }
        else if (mapSearchCamera) { mapUI.camera = mapSearchCamera; mapSearchCamera = null; mapUI.schedule(); }
      }
      details(nodes, incidents, visible); return;
    }
    $("#map-details").innerHTML = ""; detailsHTML = ""; detailsNode = null;
    const tree = (parent = "") => {
      const list = (children.get(parent) || []).filter((n) => visible.has(n.id));
      if (!list.length) return "";
      return `<ul class="tree">${list.map((saved) => {
        const node = nodes.get(saved.id);
        const hasChildren = (children.get(node.id) || []).some((n) => visible.has(n.id)), expanded = Boolean(query) || !collapsed.has(node.id);
        const resource = node.kind === "issue" ? node.available === false ? `<span class="resource">${esc(issueContext(node))}</span>` : `<a class="resource" href="${esc(issueUrl(node))}">Open ${esc(issueContext(node))}</a>` : node.kind === "pr" ? `<a class="resource" href="${esc(node.reference)}" target="_blank" rel="noopener noreferrer">Open PR ↗</a>` : node.kind === "notification" ? `<a class="resource" href="/#${esc(new URLSearchParams({view:"inbox",notice:node.reference}).toString())}">Open notification</a>` : "";
        const rel = relationships(node, nodes, incidents);
        return `<li class="node" id="${esc(node.id)}"><div class="node-row">${hasChildren ? `<button class="toggle" data-toggle="${esc(node.id)}" aria-expanded="${expanded}" aria-controls="children-${esc(node.id)}" aria-label="${expanded ? "Collapse" : "Expand"} ${esc(node.title)}">${expanded ? "▾" : "▸"}</button>` : '<span class="spacer" aria-hidden="true"></span>'}<div class="node-content"><span class="node-title">${esc(node.title)}</span><div class="meta">${node.kind !== "text" ? `<span class="badge">${esc(node.kind)}</span>` : ""}${node.state ? `<span class="state">${esc(node.state)}</span>` : ""}${node.assignee ? `<span class="assignee" title="${esc(node.assignee)}">Assigned to ${esc(assigneeName(node.assignee))}</span>` : ""}${node.alias ? `<code>${esc(node.alias)}</code>` : ""}${resource}${node.automatic ? '<span>automatic</span>' : ""}</div>${node.has_body || node.body ? node.kind === "issue" ? `<details class="resource-details" data-body-details="${esc(node.id)}" ${openBodies.has(node.id) ? "open" : ""}><summary>Issue details</summary>${bodyContent(node)}</details>` : bodyContent(node) : ""}${rel ? `<ul class="relationships" aria-label="Relationships for ${esc(node.title)}">${rel}</ul>` : ""}</div></div>${hasChildren ? `<div id="children-${esc(node.id)}" ${expanded ? "" : "hidden"}>${expanded ? tree(node.id) : ""}</div>` : ""}</li>`;
      }).join("")}</ul>`;
    };
    $("#outline").innerHTML = tree() || `<p class="empty">${query ? "No matching topics or relationships." : 'No topics yet.<br>Add the first with <code>hey-boss mm add \'Topic\' --id topic</code>'}</p>`;
    if (toggleFocus) [...document.querySelectorAll("[data-toggle]")].find((b) => b.dataset.toggle === toggleFocus)?.focus();
  }
  function reveal() {
    const target = route().get("node"); if (!target || !graph) return;
    const nodes = allNodes(); let node = nodes.get(target);
    while (node) { collapsed.delete(node.id); node = nodes.get(node.parent_id); }
    if (viewMode === "map") {
      if (!nodes.has(target)) return;
      selected = target; $("#search").value = ""; render(); mapUI.focus(target, false); $("#map-details h2")?.focus({preventScroll:true}); return;
    }
    render();
    let element = document.getElementById(target);
    if (!element && nodes.has(target) && $("#search").value) { $("#search").value = ""; render(); element = document.getElementById(target); }
    if (element) { element.classList.add("highlight"); element.scrollIntoView({ block: "center" }); element.tabIndex = -1; element.focus({ preventScroll: true }); }
  }
  async function load({ refresh = false } = {}) {
    const ticket = ++generation; controller?.abort(); controller = new AbortController();
    $("#connection").textContent = "Loading…";
    try {
      if (!csrf || refresh) {
        const response = await fetch("/api/bootstrap", { signal: controller.signal });
        const value = await response.json(); if (!response.ok || !value.ok) throw new Error(value.error?.message || "Cannot connect");
        if (ticket !== generation) return; boot = value; csrf = value.csrf; projects = value.projects;
      }
      const project = route().get("project") || boot.project.id;
      if (graph && project !== graph.project.id) $("#search").value = "";
      const response = await fetch("/api/mm", { method: "POST", signal: controller.signal, headers: { "Content-Type": "application/json", "X-Hey-Boss-CSRF": csrf }, body: JSON.stringify({ project, operation: { action: "mindmap", operation: { command: "show", body_mode: "preview" } }, request_id: null }) });
      const value = await response.json();
      // The local server renews its token after a CLI upgrade. These requests
      // are reads, so reconnect once rather than leave navigation interrupted.
      if (response.status === 403 && !refresh && ticket === generation) { csrf = ""; return load({ refresh: true }); }
      if (!response.ok || !value.ok) throw new Error(value.error?.message || "Cannot load mindmap");
      if (ticket !== generation) return;
      if (graph?.project.id !== value.project.id) { selected = null; mapSearchCamera = null; mapQuery = ""; }
      graph = value; fullBodies.clear(); bodyVersions.clear(); loadingBodies.clear(); bodyErrors.clear();
      if (!initializedProjects.has(graph.project.id)) {
        if (graph.nodes.length > 200) graph.nodes.filter((node) => !node.parent_id).forEach((node) => collapsed.add(node.id));
        initializedProjects.add(graph.project.id);
      }
      if (!projects.some((p) => p.id === graph.project.id)) projects.push(graph.project);
      $("#project").innerHTML = projects.filter((p) => p.hidden_at == null || p.id === graph.project.id).map((p) => `<option value="${esc(p.id)}" ${p.id === graph.project.id ? "selected" : ""}>${esc(p.name)}${p.hidden_at != null ? " (hidden)" : ""}</option>`).join("");
      $("#title").textContent = graph.project.name;
      $("#caption").textContent = graph.project.id;
      document.title = `${graph.project.name} · Mindmap · Hey Boss`;
      $("#connection").textContent = "Connected"; $("#error").hidden = true;
      $("#inbox-warning").hidden = graph.notifications?.available !== false;
      $("#inbox-warning").textContent = graph.notifications?.available === false ? `Notifications unavailable: ${graph.notifications.error}` : "";
      render(); reveal();
    } catch (error) {
      if (ticket !== generation || error.name === "AbortError") return;
      $("#connection").textContent = "Disconnected"; $("#error").hidden = false;
      $("#error").textContent = `${error.message}. Refresh to retry.${graph ? " Showing the last loaded outline." : ""}`;
    }
  }
  $("#project").addEventListener("change", () => { $("#search").value = ""; location.hash = new URLSearchParams({ project: $("#project").value }).toString(); });
  $("#search").addEventListener("input", render);
  $("#refresh").addEventListener("click", () => load({ refresh: true }));
  $("#expand").addEventListener("click", () => branches(true));
  $("#collapse").addEventListener("click", () => branches(false));
  const bodyClick = (event) => { const less = event.target.closest("[data-collapse-body]"); if (less) { readBody(less.dataset.collapseBody, "preview"); return; } const read = event.target.closest("[data-read-body]"); if (read) { readBody(read.dataset.readBody); return; } const button = event.target.closest("[data-toggle]"); if (button) { collapsed.has(button.dataset.toggle) ? collapsed.delete(button.dataset.toggle) : collapsed.add(button.dataset.toggle); render(); } };
  $("#outline").addEventListener("click", bodyClick); $("#map-details").addEventListener("click", bodyClick);
  $("#map-details").addEventListener("click", event => {
    const topic = event.target.closest("[data-select-topic]");
    if (topic) {
      $("#search").value = ""; const nodes = allNodes(); let node = nodes.get(topic.dataset.selectTopic);
      while (node) { collapsed.delete(node.id); node = nodes.get(node.parent_id); }
      mapUI.select(topic.dataset.selectTopic);
    } else if (event.target.closest("[data-topic-outline]")) {
      collapsed.delete(selected); setView("outline"); const target = document.getElementById(selected);
      if (target) { target.tabIndex = -1; target.focus({preventScroll:true}); target.scrollIntoView({block:"start"}); }
    }
  });
  $("#outline").addEventListener("toggle", (event) => { const details = event.target; if (details.isConnected && details.dataset.bodyDetails) { details.open ? openBodies.add(details.dataset.bodyDetails) : openBodies.delete(details.dataset.bodyDetails); } }, true);
  function setView(mode) {
    viewMode = mode; $("#view-map").setAttribute("aria-pressed", String(mode === "map")); $("#view-outline").setAttribute("aria-pressed", String(mode === "outline")); render();
    if (mode === "map") mapUI.schedule();
  }
  $("#view-map").addEventListener("click", () => setView("map")); $("#view-outline").addEventListener("click", () => setView("outline"));
  $("#map-fit").addEventListener("click", () => mapUI.fit()); $("#map-in").addEventListener("click", () => mapUI.zoom(1.2)); $("#map-out").addEventListener("click", () => mapUI.zoom(1/1.2));
  $("#close-inspector").addEventListener("click", closeInspector);
  document.addEventListener("keydown", event => { if (event.key === "Escape" && !$("#map-inspector").hidden && viewMode === "map") { event.preventDefault(); closeInspector(); } });
  $(".skip").addEventListener("click", (event) => { event.preventDefault(); setView("outline"); $("#outline").focus(); $("#outline").scrollIntoView({ block: "start" }); });
  window.addEventListener("hashchange", () => load());
  load();
})();
