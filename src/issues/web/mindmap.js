"use strict";
(() => {
  const $ = (s) => document.querySelector(s);
  const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  let csrf = "", graph = null, projects = [], generation = 0, controller = null, boot = null;
  const collapsed = new Set();
  const route = () => new URLSearchParams(location.hash.slice(1));
  const mapUrl = (project, node) => `/mm#${new URLSearchParams({ project, ...(node ? { node } : {}) })}`;
  const issueUrl = (node) => `/#${new URLSearchParams({ project: node.reference_project, issue: node.reference, ...(boot?.backend_host ? { host: boot.backend_host } : {}) })}`;
  const nodeName = (node) => `${node.project_id !== graph.project.id ? `${node.project_name || projects.find((p) => p.id === node.project_id)?.name || node.project_id} · ` : ""}${node.title}`;
  const allNodes = () => new Map([...graph.nodes, ...graph.external_nodes].map((node) => [node.id, node]));
  function relationships(node, nodes, incidents) {
    return (incidents.get(node.id) || []).map((link) => {
      const outgoing = link.from === node.id, other = nodes.get(outgoing ? link.to : link.from);
      if (!other) return "";
      const text = link.kind === "depends-on" ? (outgoing ? "Depends on" : "Required by") : link.kind === "pull-request" ? (outgoing ? "Pull request" : "Issue") : `${outgoing ? "→" : "←"} ${link.kind}`;
      return `<li><span class="relation-kind">${esc(text)}</span> <a data-map-link href="${esc(other.resource_only && other.kind === "issue" ? issueUrl(other) : mapUrl(other.project_id, other.id))}">${esc(nodeName(other))}</a>${link.description ? `<span class="description">— ${esc(link.description)}</span>` : ""}${link.automatic ? ' <span class="automatic">automatic</span>' : ""}</li>`;
    }).join("");
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
    const visible = new Set();
    for (const node of graph.nodes) {
      const rel = query ? incidents.get(node.id) || [] : [];
      if (!query || [node.title, node.body, node.alias, node.kind, node.reference, ...rel.flatMap((l) => [l.kind, l.description, nodes.get(l.from)?.title, nodes.get(l.to)?.title])].join(" ").toLowerCase().includes(query)) {
        let current = node;
        while (current && !visible.has(current.id)) { visible.add(current.id); current = nodes.get(current.parent_id); }
      }
    }
    const tree = (parent = "") => {
      const list = (children.get(parent) || []).filter((n) => visible.has(n.id));
      if (!list.length) return "";
      return `<ul class="tree">${list.map((node) => {
        const hasChildren = (children.get(node.id) || []).some((n) => visible.has(n.id)), expanded = Boolean(query) || !collapsed.has(node.id);
        const resource = node.kind === "issue" ? `<a class="resource" href="${esc(issueUrl(node))}">Open issue #${esc(node.reference)}</a>` : node.kind === "pr" ? `<a class="resource" href="${esc(node.reference)}" target="_blank" rel="noopener noreferrer">Open PR ↗</a>` : node.kind === "notification" ? `<a class="resource" href="/#${esc(new URLSearchParams({view:"inbox",notice:node.reference}).toString())}">Open notification</a>` : "";
        const rel = relationships(node, nodes, incidents);
        return `<li class="node" id="${esc(node.id)}"><div class="node-row">${hasChildren ? `<button class="toggle" data-toggle="${esc(node.id)}" aria-expanded="${expanded}" aria-controls="children-${esc(node.id)}" aria-label="${expanded ? "Collapse" : "Expand"} ${esc(node.title)}">${expanded ? "▾" : "▸"}</button>` : '<span class="spacer" aria-hidden="true"></span>'}<div class="node-content"><span class="node-title">${esc(node.title)}</span><div class="meta">${node.kind !== "text" ? `<span class="badge">${esc(node.kind)}</span>` : ""}${node.state ? `<span class="state">${esc(node.state)}</span>` : ""}${node.alias ? `<code>${esc(node.alias)}</code>` : ""}${resource}${node.automatic ? '<span>automatic</span>' : ""}</div>${node.body ? node.kind === "issue" ? `<details class="resource-details"><summary>Issue details</summary><div class="body">${node.body_html || esc(node.body)}</div></details>` : `<div class="body">${node.body_html || esc(node.body)}</div>` : ""}${rel ? `<ul class="relationships" aria-label="Relationships for ${esc(node.title)}">${rel}</ul>` : ""}</div></div>${hasChildren ? `<div id="children-${esc(node.id)}" ${expanded ? "" : "hidden"}>${tree(node.id)}</div>` : ""}</li>`;
      }).join("")}</ul>`;
    };
    $("#outline").innerHTML = tree() || `<p class="empty">${query ? "No matching topics or relationships." : 'No topics yet.<br>Add the first with <code>hey-boss mm add \'Topic\' --id topic</code>'}</p>`;
    $("#count").textContent = `${graph.nodes.length} ${graph.nodes.length === 1 ? "node" : "nodes"} · ${graph.links.length} ${graph.links.length === 1 ? "link" : "links"}`;
    $("#revision").textContent = `Map version ${graph.version}`;
    if (toggleFocus) [...document.querySelectorAll("[data-toggle]")].find((b) => b.dataset.toggle === toggleFocus)?.focus();
  }
  function reveal() {
    const target = route().get("node"); if (!target || !graph) return;
    const nodes = allNodes(); let node = nodes.get(target);
    while (node) { collapsed.delete(node.id); node = nodes.get(node.parent_id); }
    render();
    const element = document.getElementById(target);
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
      const response = await fetch("/api/mm", { method: "POST", signal: controller.signal, headers: { "Content-Type": "application/json", "X-Hey-Boss-CSRF": csrf }, body: JSON.stringify({ project, operation: { action: "mindmap", operation: { command: "show" } }, request_id: null }) });
      const value = await response.json();
      // The local server renews its token after a CLI upgrade. These requests
      // are reads, so reconnect once rather than leave navigation interrupted.
      if (response.status === 403 && !refresh && ticket === generation) { csrf = ""; return load({ refresh: true }); }
      if (!response.ok || !value.ok) throw new Error(value.error?.message || "Cannot load mindmap");
      if (ticket !== generation) return;
      graph = value;
      if (!projects.some((p) => p.id === graph.project.id)) projects.push(graph.project);
      $("#project").innerHTML = projects.map((p) => `<option value="${esc(p.id)}" ${p.id === graph.project.id ? "selected" : ""}>${esc(p.name)}</option>`).join("");
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
  $("#expand").addEventListener("click", () => { collapsed.clear(); render(); });
  $("#collapse").addEventListener("click", () => { if (graph) graph.nodes.forEach((n) => collapsed.add(n.id)); render(); });
  $("#outline").addEventListener("click", (event) => { const button = event.target.closest("[data-toggle]"); if (button) { collapsed.has(button.dataset.toggle) ? collapsed.delete(button.dataset.toggle) : collapsed.add(button.dataset.toggle); render(); } });
  $(".skip").addEventListener("click", (event) => { event.preventDefault(); $("#outline").focus(); $("#outline").scrollIntoView({ block: "start" }); });
  window.addEventListener("hashchange", () => load());
  load();
})();
