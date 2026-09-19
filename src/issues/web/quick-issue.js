"use strict";
const HeyBossQuickIssue = (() => {
  // Resolve whole mention tokens, never substrings or email addresses. Full IDs
  // take precedence over names; repeated mentions must select the same project.
  function parse(text, projects, current) {
    let title = "", selected = null;
    for (let i = 0; i < text.length;) {
      if (text[i] === "\\" && text[i + 1] === "@") { title += "@"; i += 2; continue; }
      if (text[i] !== "@" || (i && /[\p{L}\p{N}_.+\-@]/u.test(text[i - 1]))) { title += text[i++]; continue; }
      let end = i + 1, name;
      const quote = text[end];
      if (quote === '"' || quote === "'") {
        name = ""; ++end;
        while (end < text.length && text[end] !== quote) {
          if (text[end] === "\\" && [quote, "\\"].includes(text[end + 1])) ++end;
          name += text[end++];
        }
        if (text[end] !== quote) throw new Error("Close the quote around the project name.");
        ++end;
        if (/[\p{L}\p{N}_:/\-]/u.test(text[end] || "")) throw new Error("Separate the project mention from the title.");
      } else {
        const token = text.slice(end).match(/^[\p{L}\p{N}_:/\.\-]+/u)?.[0] || "";
        name = token.replace(/\.+$/, ""); end += name.length;
      }
      if (!name) throw new Error("Enter a project after @, or use \\@ for literal text.");
      const fold = value => value.normalize("NFC").toLocaleLowerCase();
      const exact = projects.filter(p => p.id === name);
      const matches = exact.length ? exact : projects.filter(p => fold(p.name) === fold(name) || fold(p.id) === fold(name));
      if (!matches.length) throw new Error(`Unknown project @${name}. Use a known project name or full ID.`);
      if (matches.length !== 1) throw new Error(`Project @${name} is ambiguous. Use its full project ID.`);
      if (selected && selected.id !== matches[0].id) throw new Error("Use mentions for only one project per issue.");
      selected = matches[0]; i = end;
    }
    title = title.replace(/\s+/g, " ").trim();
    if (!title) throw new Error("Enter an issue title.");
    const project = selected || current;
    if (!project) throw new Error("Choose a project or include an @project mention.");
    return {title, project};
  }

  function init() {
    const dialog = document.getElementById("quick-issue-dialog");
    if (!dialog) return;
    const input = document.getElementById("quick-issue-title"),
      error = document.getElementById("quick-issue-error"),
      context = document.getElementById("quick-issue-context"),
      submit = document.getElementById("quick-issue-submit"),
      bottom = document.getElementById("quick-issue-bottom"),
      status = document.getElementById("quick-issue-status");
    let projects = [], current = null, csrf, host, ready = false, saving = false,
      sequence = 0, pending = null, previousFocus, statusTimer;
    const fail = message => { error.textContent = message; error.hidden = false; };
    function preview() {
      error.hidden = true;
      submit.disabled = saving || !ready || !input.value.trim();
      if (!ready) return;
      try {
        const value = parse(input.value, projects, current);
        context.textContent = `Create in ${value.project.name}`;
      } catch (e) {
        context.textContent = current ? `Create in ${current.name} · @project to switch` : "@project to choose a project";
      }
    }
    async function post(operation, project, request_id = null) {
      const response = await fetch("/api/action", {
        method: "POST", headers: {"Content-Type":"application/json", "X-Hey-Boss-CSRF":csrf},
        body: JSON.stringify({project, operation, host, request_id}),
        signal: AbortSignal.timeout(15000),
      });
      const value = await response.json();
      if (!response.ok || !value.ok) throw new Error(value.error?.message || "Could not create the issue.");
      return value;
    }
    async function open() {
      if (dialog.open) { input.focus(); return; }
      previousFocus = document.activeElement;
      const generation = ++sequence;
      ready = false; projects = []; current = null;
      context.textContent = "Loading projects…"; error.hidden = true; submit.disabled = true;
      dialog.showModal(); input.focus();
      const route = new URLSearchParams(location.hash.slice(1));
      const projectId = HeyBossUI.projectId(null);
      host = route.get("host") || null;
      try {
        const response = await fetch("/api/bootstrap", {signal:AbortSignal.timeout(10000)});
        let value = await response.json();
        if (!response.ok || !value.ok) throw new Error(value.error?.message || "Could not load projects.");
        if (generation !== sequence) return;
        csrf = value.csrf;
        if (host && host !== value.backend_host) value = await post({action:"projects", include_hidden:true}, projectId);
        if (generation !== sequence) return;
        projects = value.projects;
        current = projects.find(p => p.id === projectId) || (projectId ? null : value.project);
        ready = true; preview();
      } catch (e) { if (generation === sequence) { context.textContent = "Unable to load projects · close and retry"; fail(e.message); } }
    }
    function close() {
      if (saving) return;
      ++sequence; dialog.close();
      if (previousFocus?.isConnected) previousFocus.focus({preventScroll:true});
    }
    function openFromLink() {
      const route = new URLSearchParams(location.hash.slice(1));
      if (route.get("quick-issue") !== "1") return;
      route.delete("quick-issue");
      history.replaceState(null, "", `${location.pathname}${location.search}${route.size ? "#" + route : ""}`);
      open();
    }
    window.addEventListener("hashchange", openFromLink);
    openFromLink();
    document.getElementById("quick-issue-open").onclick = open;
    document.getElementById("quick-issue-close").onclick = close;
    input.oninput = preview;
    dialog.addEventListener("cancel", event => { event.preventDefault(); close(); });
    dialog.addEventListener("click", event => {
      if (event.target !== dialog) return;
      const bounds = dialog.getBoundingClientRect();
      if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) close();
    });
    document.addEventListener("keydown", event => {
      if ((event.metaKey || event.ctrlKey) && event.shiftKey && !event.altKey && event.key.toLowerCase() === "k") {
        event.preventDefault(); event.stopImmediatePropagation(); if (!event.repeat) open(); return;
      }
      // A modal can be opened above another form without sending its shortcuts.
      if (!dialog.open) return;
      if ((event.metaKey || event.ctrlKey) && event.shiftKey && !event.altKey && event.key.toLowerCase() === "b") {
        event.preventDefault(); event.stopImmediatePropagation();
        if (!event.repeat && !event.isComposing && !saving) bottom.checked = !bottom.checked;
        return;
      }
      if (event.key === "Escape") { event.preventDefault(); event.stopImmediatePropagation(); close(); }
      else if (event.key === "Enter" && event.target === input) {
        event.preventDefault(); event.stopImmediatePropagation();
        if (!event.isComposing && !event.repeat && !saving && ready) document.getElementById("quick-issue-form").requestSubmit();
      }
    }, true);
    document.getElementById("quick-issue-form").onsubmit = async event => {
      event.preventDefault();
      if (!ready || saving) return;
      let parsed;
      try { parsed = parse(input.value, projects, current); } catch (e) { fail(e.message); input.focus(); return; }
      const operation = {action:"create", title:parsed.title, body:"", labels:[], at_top:!bottom.checked};
      const key = JSON.stringify([host, parsed.project.id, operation]);
      if (pending?.key !== key) pending = {key, id:crypto.randomUUID()};
      saving = true; input.disabled = true; bottom.disabled = true; submit.disabled = true; error.hidden = true; context.textContent = "Creating…";
      try {
        const value = await post(operation, parsed.project.id, pending.id);
        const savedHost = host;
        pending = null; input.value = ""; bottom.checked = false; saving = false; close();
        const link = document.createElement("a");
        link.href = `/#${new URLSearchParams({project:value.project.id, issue:value.issue.number, ...(savedHost ? {host:savedHost} : {})})}`;
        link.textContent = `Created #${value.issue.number} in ${value.project.name}`;
        status.replaceChildren(link); status.hidden = false;
        clearTimeout(statusTimer); statusTimer = setTimeout(() => status.hidden = true, 10000);
        window.dispatchEvent(new CustomEvent("hey-boss-issue-created", {detail:value}));
      } catch (e) { fail(`${e.message} Your title is preserved; retry to submit safely.`); }
      finally { saving = false; input.disabled = false; bottom.disabled = false; if (dialog.open) { preview(); error.hidden = false; input.focus(); } }
    };
  }
  if (typeof document !== "undefined") init();
  return {parse};
})();
if (typeof module !== "undefined") module.exports = HeyBossQuickIssue;
