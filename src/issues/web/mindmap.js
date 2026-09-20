"use strict";
(() => {
  const $ = (s) => document.querySelector(s);
  const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  let csrf = "", graph = null, projects = [], generation = 0, controller = null, boot = null;
  HeyBossUI.icons();
  const projectPicker = new HeyBossUI.ProjectPicker({
    onSelect(project) { $("#search").value = ""; location.hash = new URLSearchParams({project}).toString(); },
  });
  const collapsed = new Set(), openBodies = new Set();
  let viewMode = "map", selected = null, mapQuery = "", mapSearchCamera = null;
  let detailsHTML = "", detailsArtifacts = "", detailsNode = null;
  let relationshipPage = 0;
  let renderingIndex = null;
  let searchQuery = "", searchMatches = [], searchHit = null, mapHit = null;
  const fullBodies = new Map(), bodyVersions = new Map(), loadingBodies = new Map(), bodyErrors = new Map(), initializedProjects = new Set();
  const route = () => new URLSearchParams(location.hash.slice(1));
  const mapUrl = (project, node) => `/mm${document.documentElement.classList.contains("mindmap-focus") ? "?focus=1" : ""}#${new URLSearchParams({ project, ...(node ? { node } : {}) })}`;
  const issueUrl = (node) => `/#${new URLSearchParams({ project: node.reference_project, issue: node.reference, ...(boot?.backend_host ? { host: boot.backend_host } : {}) })}`;
  const issueContext = (node) => `${node.reference_project !== graph.project.id ? `${node.reference_project_name || projects.find((p) => p.id === node.reference_project)?.name || node.reference_project} · ` : ""}issue #${node.reference}`;
  const nodeName = (node) => `${node.project_id !== graph.project.id ? `${node.project_name || projects.find((p) => p.id === node.project_id)?.name || node.project_id} · ` : ""}${HeyBossMap.displayTitle(node)}`;
  const assigneeName = (id) => {
    if (!id) return "Unassigned";
    if (id === "human:boss") return graph?.boss?.name || boot?.boss?.name || "Boss";
    if (id.startsWith("codex:")) return `Codex · ${id.slice(6, 14)}`;
    if (id.startsWith("claude:")) return `Claude · ${id.slice(7, 15)}`;
    return id.replace(/^human:/, "").split("@")[0];
  };
  const allNodes = () => new Map([...graph.nodes, ...graph.external_nodes].map((node) => [node.id, fullBodies.get(node.id) || node]));
  function relationships(node, nodes, links) {
    return links.map((link) => {
      const outgoing = link.from === node.id, other = nodes.get(outgoing ? link.to : link.from);
      if (!other) return "";
      const text = link.kind === "depends-on" ? (outgoing ? "Depends on" : "Required by") : link.kind === "pull-request" ? (outgoing ? "Pull request" : "Issue") : `${outgoing ? "→" : "←"} ${link.kind}`;
      return `<li><span class="relation-kind">${esc(text)}</span> <a data-map-link data-rel-key="${esc(JSON.stringify([link.from,link.to,link.kind,Boolean(link.automatic)]))}" href="${esc(other.resource_only && other.kind === "issue" ? issueUrl(other) : mapUrl(other.project_id, other.id))}">${esc(nodeName(other))}</a>${other.kind === "issue" ? ` <span class="description">(${esc(issueContext(other))})</span> ` : ""}${link.description ? `<span class="description">— ${esc(link.description)}</span>` : ""}${link.automatic ? ' <span class="automatic">automatic</span>' : ""}</li>`;
    }).join("");
  }
  function captureFocus(container) {
    const active = document.activeElement;
    if (!container.contains(active)) return null;
    const attribute = ["id","data-toggle","data-read-body","data-collapse-body","data-select-topic","data-topic-outline","data-rel-page","data-rel-key","href"].find(name => active.hasAttribute(name));
    return {attribute,value:attribute ? active.getAttribute(attribute) : null,scope:active.closest(".node, #map-details > div[id]")?.id,heading:active.matches("h2[tabindex]"),summary:active.tagName === "SUMMARY" ? active.parentElement.dataset.bodyDetails : null};
  }
  function focusIdentity() {
    const state = captureFocus(document.body);
    return state?.scope && (state.attribute || state.heading || state.summary) ? JSON.stringify(state) : document.activeElement;
  }
  function restoreFocus(container, state) {
    if (!state) return;
    const owner = state.scope && document.getElementById(state.scope);
    const scope = owner && container.contains(owner) ? owner : container;
    const selector = state.attribute ? `[${state.attribute}="${CSS.escape(state.value)}"]` : null;
    let target = state.heading ? scope.querySelector("h2[tabindex]") : state.summary ? scope.querySelector(`[data-body-details="${CSS.escape(state.summary)}"] > summary`) : selector ? scope.matches(selector) ? scope : scope.querySelector(selector) : null;
    if (!target || target.disabled) target = (state.attribute === "data-rel-page" ? scope.querySelector("[data-rel-page]:not(:disabled)") : null) || scope.querySelector("h2[tabindex]") || scope;
    if (target.tabIndex < 0) target.tabIndex = -1;
    target.focus({preventScroll:true});
  }
  function issueMetadata(node) {
    if (node.kind !== "issue") return "";
    const title = node.display_label ? `<p class="original-title">${esc(node.original_title)}</p>` : "";
    const labels = node.labels?.length ? `<p class="issue-labels">Labels: ${node.labels.map(esc).join(", ")}</p>` : "";
    return title + labels;
  }
  function bodyContent(node) {
    const full = fullBodies.has(node.id);
    const error = bodyErrors.has(node.id) ? `<p class="body-error" role="alert">${esc(bodyErrors.get(node.id))}</p>` : "";
    return `${full ? `<button class="read-body" data-collapse-body="${esc(node.id)}" ${loadingBodies.has(node.id) ? "disabled" : ""}>Show less</button>${error}` : ""}<div class="body" id="body-${esc(node.id)}" tabindex="-1">${node.body_html || esc(node.body)}</div>${node.body_truncated ? `<button class="read-body" data-read-body="${esc(node.id)}" ${loadingBodies.has(node.id) ? "disabled" : ""}>${loadingBodies.has(node.id) ? "Loading…" : "Read full text"}</button>` : ""}${full ? "" : error}`;
  }
  const mapUI = new HeyBossMap.Mindmap($("#mindmap"), {
    select(id) { selected = id; render(); $("#map-details h2")?.focus({preventScroll:true}); },
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
      const card = mapUI.cards.get(target)?.querySelector("[data-map-node]");
      (card || $("#mindmap")).focus({preventScroll:true});
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
    if (detailsNode !== node.id) relationshipPage = 0;
    const links = (incidents.get(node.id) || []).filter(link => nodes.has(link.from === node.id ? link.to : link.from));
    relationshipPage = Math.max(0, Math.min(relationshipPage, Math.ceil(links.length / 50) - 1));
    const start = relationshipPage * 50, end = Math.min(start + 50, links.length);
    const rel = relationships(node, nodes, links.slice(start, end));
    const pager = links.length > 50 ? `<div class="relationship-navigation" role="group" aria-label="Relationship pages"><button type="button" data-rel-page="previous" aria-label="Previous relationships" ${start === 0 ? "disabled" : ""}>‹</button><span role="status">${start + 1}–${end} of ${links.length.toLocaleString()}</span><button type="button" data-rel-page="next" aria-label="Next relationships" ${end === links.length ? "disabled" : ""}>›</button></div>` : "";
    const children = graph.nodes.filter(n => n.parent_id === node.id);
    const topics = children.length ? `<section class="topic-children" aria-label="Child topics"><h3>Topics <span>${children.length}</span></h3><ul>${children.slice(0,50).map(n => `<li><button type="button" data-select-topic="${esc(n.id)}">${esc(HeyBossMap.displayTitle(nodes.get(n.id)))}</button></li>`).join("")}</ul>${children.length > 50 ? '<button type="button" data-topic-outline>View all in outline</button>' : ""}</section>` : "";
    const html = `<div id="${esc(node.id)}"><h2 tabindex="-1">${esc(HeyBossMap.displayTitle(node))}</h2><div class="meta">${HeyBossMap.kindIcon(node.kind)}${node.state ? `<span>${esc(node.state)}</span>` : ""}${node.alias ? `<code>${esc(node.alias)}</code>` : ""}${node.assignee ? `<span>Assigned to ${esc(assigneeName(node.assignee))}</span>` : ""}${node.available === false ? "<span>Resource unavailable</span>" : ""}</div>${resource}<section id="node-artifacts"></section><section id="node-attachments"></section>${issueMetadata(node)}${node.has_body || node.body ? bodyContent(node) : ""}${topics}${rel ? `<section class="topic-relationships">${pager}<ul class="relationships" aria-label="Relationships for ${esc(HeyBossMap.displayTitle(node))}">${rel}</ul></section>` : ""}</div>`;
    const artifacts = JSON.stringify(node.artifacts || []);
    if (html === detailsHTML && artifacts === detailsArtifacts) return;
    const same = detailsNode === node.id, focus = same ? captureFocus(container) : null;
    container.innerHTML = html; detailsHTML = html; detailsArtifacts = artifacts; detailsNode = node.id;
    HeyBossAttachments.mount($("#node-attachments"), {project:node.project_id,target:{kind:"node",id:node.id},csrf});
    HeyBossArtifacts.mount($("#node-artifacts"), {project:node.project_id,node:node.id,csrf,artifacts:node.artifacts || []});
    if (!same) container.scrollTop = 0;
    restoreFocus(container, focus);
  }
  async function readBody(id, mode = "full") {
    if (loadingBodies.has(id) || !graph) return;
    const ticket = generation, project = graph.project.id, signal = controller.signal, query = $("#search").value;
    loadingBodies.set(id, ticket); bodyErrors.delete(id); render();
    const readingFocus = focusIdentity();
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
      renderingIndex = null;
      const retainReadingFocus = focusIdentity() === readingFocus;
      loadingBodies.delete(id); render();
      if (!document.getElementById(id) && query && $("#search").value === query) {
        $("#search").value = "";
        const nodes = allNodes(); let node = nodes.get(id);
        while (node) { collapsed.delete(node.id); node = nodes.get(node.parent_id); }
        render();
      }
      const focus = document.getElementById(`body-${id}`) || document.getElementById(id);
      if (focus && retainReadingFocus && document.activeElement !== $("#search")) { focus.tabIndex = -1; focus.focus({preventScroll:true}); }
    } catch (error) {
      if (ticket !== generation || error.name === "AbortError") return;
      bodyErrors.set(id,`${error.message}. ${mode === "preview" ? "Try Show less again." : "Retry to load the full text."}`);
    } finally {
      if (loadingBodies.get(id) === ticket) {
        const retainReadingFocus = focusIdentity() === readingFocus;
        loadingBodies.delete(id); render();
        if (bodyErrors.has(id) && retainReadingFocus && document.activeElement !== $("#search")) document.querySelector(`[data-${mode === "preview" ? "collapse" : "read"}-body="${id}"]`)?.focus({preventScroll:true});
      }
    }
  }
  function render() {
    if (!graph) return;
    const outlineFocus = captureFocus($("#outline"));
    const { nodes, incidents, documents } = renderingIndex ||= HeyBossMap.indexGraph([...allNodes().values()], graph.links, assigneeName);
    const query = $("#search").value.trim().toLowerCase();
    const visible = new Set(), matched = new Set();
    for (const node of graph.nodes) {
      if (!query || documents.get(node.id).includes(query)) {
        matched.add(node.id);
        let current = node;
        while (current && !visible.has(current.id)) { visible.add(current.id); current = nodes.get(current.parent_id); }
      }
    }
    searchMatches = query ? [...matched] : [];
    if (query !== searchQuery || !matched.has(searchHit)) searchHit = searchMatches[0] || null;
    searchQuery = query;
    $("#search-navigation").hidden = !query;
    $("#search-status").textContent = searchMatches.length ? `${searchMatches.indexOf(searchHit) + 1} of ${searchMatches.length}` : query ? "No matches" : "";
    $("#search-status").title = searchHit ? HeyBossMap.displayTitle(nodes.get(searchHit)) : "";
    $("#search-previous").disabled = $("#search-next").disabled = searchMatches.length < 2;
    $("#count").textContent = `${graph.nodes.length} ${graph.nodes.length === 1 ? "node" : "nodes"} · ${graph.links.length} ${graph.links.length === 1 ? "link" : "links"}`;
    $("#revision").textContent = `Map version ${graph.version}`;
    document.body.dataset.mindmapView = viewMode;
    $("#outline").hidden = viewMode !== "outline"; $("#map-panel").hidden = viewMode !== "map";
    if (viewMode === "map") {
      $("#outline").innerHTML = "";
      if (mapQuery !== query && query && !mapQuery) mapSearchCamera = {...mapUI.camera, viewportWidth:$("#mindmap").clientWidth, viewportHeight:$("#mindmap").clientHeight};
      mapUI.update(graph.nodes.map(n => nodes.get(n.id)), {collapsed, matches:query ? visible : null, hits:query ? matched : null, currentHit:searchHit, project:graph.project, selected, links:graph.links, assigneeName});
      if (mapQuery !== query || mapHit !== searchHit) {
        const changedQuery = mapQuery !== query;
        mapQuery = query;
        mapHit = searchHit;
        if (query) { if (changedQuery) mapUI.fit(); if (searchHit) mapUI.focus(searchHit, false); }
        else if (mapSearchCamera) {
          mapUI.camera = {scale:mapSearchCamera.scale,x:mapSearchCamera.x + ($("#mindmap").clientWidth - mapSearchCamera.viewportWidth) / 2,y:mapSearchCamera.y + ($("#mindmap").clientHeight - mapSearchCamera.viewportHeight) / 2};
          mapSearchCamera = null; mapUI.schedule();
        }
      }
      details(nodes, incidents, visible); return;
    }
    $("#map-details").innerHTML = ""; detailsHTML = ""; detailsNode = null;
    const children = new Map();
    for (const node of graph.nodes) { const key = node.parent_id || ""; if (!children.has(key)) children.set(key, []); children.get(key).push(node); }
    const tree = (parent = "") => {
      const list = (children.get(parent) || []).filter((n) => visible.has(n.id));
      if (!list.length) return "";
      return `<ul class="tree">${list.map((saved) => {
        const node = nodes.get(saved.id);
        const hasChildren = (children.get(node.id) || []).some((n) => visible.has(n.id)), expanded = Boolean(query) || !collapsed.has(node.id);
        const resource = node.kind === "issue" ? node.available === false ? `<span class="resource">${esc(issueContext(node))}</span>` : `<a class="resource" href="${esc(issueUrl(node))}">Open ${esc(issueContext(node))}</a>` : node.kind === "pr" ? `<a class="resource" href="${esc(node.reference)}" target="_blank" rel="noopener noreferrer">Open PR ↗</a>` : node.kind === "notification" ? `<a class="resource" href="/#${esc(new URLSearchParams({view:"inbox",notice:node.reference}).toString())}">Open notification</a>` : "";
        const rel = relationships(node, nodes, incidents.get(node.id) || []);
        return `<li class="node${query && node.id === searchHit ? " highlight" : ""}" id="${esc(node.id)}"><div class="node-row">${hasChildren ? `<button class="toggle" data-toggle="${esc(node.id)}" aria-expanded="${expanded}" aria-controls="children-${esc(node.id)}" aria-label="${expanded ? "Collapse" : "Expand"} ${esc(HeyBossMap.displayTitle(node))}">${expanded ? "▾" : "▸"}</button>` : '<span class="spacer" aria-hidden="true"></span>'}<div class="node-content"><span class="node-title">${esc(HeyBossMap.displayTitle(node))}</span><div class="meta">${HeyBossMap.kindIcon(node.kind)}${node.state ? `<span class="state">${esc(node.state)}</span>` : ""}${node.assignee ? `<span class="assignee" title="${esc(node.assignee)}">Assigned to ${esc(assigneeName(node.assignee))}</span>` : ""}${node.alias ? `<code>${esc(node.alias)}</code>` : ""}${resource}${node.automatic ? '<span>automatic</span>' : ""}</div>${issueMetadata(node)}${node.has_body || node.body ? node.kind === "issue" ? `<details class="resource-details" data-body-details="${esc(node.id)}" ${openBodies.has(node.id) ? "open" : ""}><summary>Issue details</summary>${bodyContent(node)}</details>` : bodyContent(node) : ""}${rel ? `<ul class="relationships" aria-label="Relationships for ${esc(HeyBossMap.displayTitle(node))}">${rel}</ul>` : ""}</div></div>${hasChildren ? `<div id="children-${esc(node.id)}" ${expanded ? "" : "hidden"}>${expanded ? tree(node.id) : ""}</div>` : ""}</li>`;
      }).join("")}</ul>`;
    };
    $("#outline").innerHTML = tree() || `<p class="empty">${query ? "No matching topics or relationships." : 'No topics yet.<br>Add the first with <code>hey-boss mm add \'Topic\' --id topic</code>'}</p>`;
    restoreFocus($("#outline"), outlineFocus);
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
    $("#connection span").textContent = "Loading…";
    try {
      if (!csrf || refresh) {
        const response = await fetch("/api/bootstrap", { signal: controller.signal });
        const value = await response.json(); if (!response.ok || !value.ok) throw new Error(value.error?.message || "Cannot connect");
        if (ticket !== generation) return; boot = value; csrf = value.csrf; projects = value.projects;
      }
      const project = HeyBossUI.projectId(boot.project.id);
      if (graph && project !== graph.project.id) $("#search").value = "";
      const response = await fetch("/api/mm", { method: "POST", signal: controller.signal, headers: { "Content-Type": "application/json", "X-Hey-Boss-CSRF": csrf }, body: JSON.stringify({ project, operation: { action: "mindmap", operation: { command: "show", body_mode: "preview" } }, request_id: null }) });
      const value = await response.json();
      // The local server renews its token after a CLI upgrade. These requests
      // are reads, so reconnect once rather than leave navigation interrupted.
      if (response.status === 403 && !refresh && ticket === generation) { csrf = ""; return load({ refresh: true }); }
      if (!response.ok || !value.ok) throw new Error(value.error?.message || "Cannot load mindmap");
      if (ticket !== generation) return;
      if (graph?.project.id !== value.project.id) { selected = null; mapSearchCamera = null; mapQuery = ""; }
      graph = value; renderingIndex = null; fullBodies.clear(); bodyVersions.clear(); loadingBodies.clear(); bodyErrors.clear();
      if (!initializedProjects.has(graph.project.id)) {
        if (graph.nodes.length > 200) graph.nodes.filter((node) => !node.parent_id).forEach((node) => collapsed.add(node.id));
        initializedProjects.add(graph.project.id);
      }
      if (!projects.some((p) => p.id === graph.project.id)) projects.push(graph.project);
      projectPicker.update(projects, graph.project);
      $("#title").textContent = graph.project.name;
      $("#caption").textContent = "Topics, work and dependencies in one map.";
      document.title = `${graph.project.name} · Mindmap · Hey Boss`;
      $("#connection").classList.remove("offline"); $("#connection span").textContent = "Connected"; $("#error").hidden = true;
      $("#inbox-warning").hidden = graph.notifications?.available !== false;
      $("#inbox-warning").textContent = graph.notifications?.available === false ? `Notifications unavailable: ${graph.notifications.error}` : "";
      render(); reveal();
    } catch (error) {
      if (ticket !== generation || error.name === "AbortError") return;
      $("#connection").classList.add("offline"); $("#connection span").textContent = "Disconnected"; $("#error").hidden = false;
      $("#error").textContent = `${error.message}. Refresh to retry.${graph ? " Showing the last loaded outline." : ""}`;
    }
  }

  $("#search").addEventListener("input", render);
  function searchMatch(step) {
    if (!searchMatches.length) return;
    searchHit = searchMatches[(searchMatches.indexOf(searchHit) + step + searchMatches.length) % searchMatches.length];
    render();
    if (viewMode === "outline") document.getElementById(searchHit)?.scrollIntoView({block:"center"});
  }
  $("#search").addEventListener("keydown", event => {
    if (event.key === "Enter" && !event.isComposing && searchMatches.length) { event.preventDefault(); searchMatch(event.shiftKey ? -1 : 1); }
  });
  $("#search-previous").addEventListener("click", () => searchMatch(-1));
  $("#search-next").addEventListener("click", () => searchMatch(1));
  $("#refresh").addEventListener("click", () => load({ refresh: true }));
  $("#expand").addEventListener("click", () => branches(true));
  $("#collapse").addEventListener("click", () => branches(false));
  // Safari does not focus mouse-clicked buttons; anchor each read before rendering.
  const bodyClick = (event) => { const less = event.target.closest("[data-collapse-body]"); if (less) { less.focus({preventScroll:true}); readBody(less.dataset.collapseBody, "preview"); return; } const read = event.target.closest("[data-read-body]"); if (read) { read.focus({preventScroll:true}); readBody(read.dataset.readBody); return; } const button = event.target.closest("[data-toggle]"); if (button) { collapsed.has(button.dataset.toggle) ? collapsed.delete(button.dataset.toggle) : collapsed.add(button.dataset.toggle); render(); } };
  $("#outline").addEventListener("click", bodyClick); $("#map-details").addEventListener("click", bodyClick);
  $("#map-details").addEventListener("click", event => {
    const page = event.target.closest("[data-rel-page]");
    if (page) {
      relationshipPage += page.dataset.relPage === "next" ? 1 : -1; render();
      const container = $("#map-details"), section = container.querySelector(".topic-relationships");
      if (section) container.scrollTop += section.getBoundingClientRect().top - container.getBoundingClientRect().top;
      return;
    }
    const topic = event.target.closest("[data-select-topic]");
    if (topic) {
      $("#search").value = ""; const nodes = allNodes(); let node = nodes.get(topic.dataset.selectTopic);
      while (node) { collapsed.delete(node.id); node = nodes.get(node.parent_id); }
      mapUI.select(topic.dataset.selectTopic);
      mapUI.focus(topic.dataset.selectTopic, false);
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
  document.addEventListener("keydown", event => { if (event.defaultPrevented) return; if (event.key === "Escape" && !$("#map-inspector").hidden && viewMode === "map") { event.preventDefault(); closeInspector(); } });
  $(".skip, .skip-link").addEventListener("click", (event) => { event.preventDefault(); setView("outline"); $("#outline").focus(); $("#outline").scrollIntoView({ block: "start" }); });
  window.addEventListener("hashchange", () => load());
  load();
})();
