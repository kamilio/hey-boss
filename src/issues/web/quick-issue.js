"use strict";
const HeyBossQuickIssue = (() => {
  const taskKind = (labels = []) => labels.some(label => ["task:plan", "task:research"].includes(label)) ? "plan" : "implement";
  const taskLabels = (labels, kind) => [...new Set(labels.filter(label => !["task:plan", "task:research"].includes(label))), ...(kind === "plan" ? [`task:${kind}`] : [])];
  const fold = value => value.normalize("NFC").toLocaleLowerCase();
  // Scan whole tokens so email addresses, escaped @ signs and @ inside quotes
  // cannot open another picker. Offsets match the input's selection offsets.
  function mentionAt(text, caret) {
    for (let i = 0; i < text.length;) {
      if (text[i] === "\\" && text[i + 1] === "@") { i += 2; continue; }
      if (text[i] !== "@" || (i && /[\p{L}\p{N}_.+\-@]/u.test(text[i - 1]))) { ++i; continue; }
      const start = i++, quote = text[i];
      let query = "", closed = false;
      if (quote === '"' || quote === "'") {
        ++i;
        while (i < text.length && text[i] !== quote) {
          if (text[i] === "\\" && [quote, "\\"].includes(text[i + 1])) ++i;
          if (i < caret) query += text[i];
          ++i;
        }
        if (text[i] === quote) { ++i; closed = true; }
      } else {
        const token = (text.slice(i).match(/^[\p{L}\p{N}_:/\.\-]+/u)?.[0] || "").replace(/\.+$/, "");
        query = text.slice(i,Math.max(i,Math.min(caret,i + token.length)));
        i += token.length;
      }
      if (caret > start && caret <= i && !(closed && caret === i)) return {start, end:i, query};
    }
    return null;
  }
  function suggestions(projects, query) {
    query = fold(query);
    const rank = p => fold(p.name) === query ? 0 : fold(p.name).startsWith(query) ? 1 : fold(p.id).startsWith(query) ? 2 : 3;
    return projects.filter(p => fold(p.name).includes(query) || fold(p.id).includes(query))
      .sort((a,b) => rank(a) - rank(b) || a.name.localeCompare(b.name) || a.id.localeCompare(b.id)).slice(0,8);
  }
  function completeMention(text, mention, project, projects) {
    const unique = projects.filter(p => fold(p.name) === fold(project.name) || fold(p.id) === fold(project.name));
    let name = unique.length === 1 && unique[0].id === project.id ? project.name : project.id;
    if (!/^[\p{L}\p{N}_:/\.\-]+$/u.test(name) || name.endsWith('.')) name = `"${name.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
    const suffix = text.slice(mention.end), space = !suffix || /^[\p{L}\p{N}_:/\-]/u.test(suffix) ? " " : "";
    const prefix = text.slice(0,mention.start) + "@" + name + space;
    return {text:prefix + suffix, caret:prefix.length + (/^\s/.test(suffix) ? 1 : 0)};
  }
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
      kind = document.getElementById("quick-issue-kind"),
      status = document.getElementById("quick-issue-status");
    const picker = document.getElementById("quick-issue-project-picker"),
      list = document.getElementById("quick-issue-projects"),
      pickerStatus = document.getElementById("quick-issue-project-status");
    let projects = [], current = null, csrf, host, ready = false, saving = false,
      sequence = 0, pending = null, previousFocus, statusTimer;
    let mention = null, matches = [], active = 0, dismissed = false, composing = false;
    function hidePicker() {
      picker.hidden = true; mention = null; matches = [];
      input.setAttribute("aria-expanded", "false"); input.removeAttribute("aria-activedescendant");
    }
    function highlight() {
      [...list.children].forEach((option,i) => option.setAttribute("aria-selected", String(i === active)));
      const option = list.children[active];
      if (option) { input.setAttribute("aria-activedescendant",option.id); option.scrollIntoView({block:"nearest"}); }
      else input.removeAttribute("aria-activedescendant");
    }
    function updatePicker() {
      if (!ready || saving || composing || dismissed || !dialog.open || document.activeElement !== input || input.selectionStart !== input.selectionEnd) { hidePicker(); return; }
      mention = mentionAt(input.value,input.selectionStart);
      if (!mention) { hidePicker(); return; }
      matches = suggestions(projects,mention.query); active = 0; list.replaceChildren();
      matches.forEach((project,i) => {
        const option = document.createElement("div");
        option.id = `quick-issue-project-${i}`; option.className = "quick-issue-project"; option.setAttribute("role","option");
        const icon = document.createElement("span"); icon.className = "quick-issue-project-icon"; icon.textContent = "@"; icon.setAttribute("aria-hidden","true");
        const text = document.createElement("span"); text.className = "quick-issue-project-text";
        const name = document.createElement("strong"); name.textContent = project.name;
        const id = document.createElement("span"); id.textContent = "Project name";
        text.append(name,id); option.append(icon,text);
        if (project.id === current?.id) { const badge = document.createElement("span"); badge.className = "quick-issue-project-current"; badge.textContent = "Current"; option.append(badge); }
        // Keep input focus on mouse selection without suppressing touch clicks.
        option.addEventListener("mousedown",event => event.preventDefault());
        option.addEventListener("click",() => { active = i; choose(); });
        list.append(option);
      });
      pickerStatus.textContent = matches.length ? `${matches.length} project${matches.length === 1 ? "" : "s"} · ↑↓ to browse · Enter or Tab to select` : "No matching projects · try a name or project ID";
      picker.hidden = false; input.setAttribute("aria-expanded","true"); highlight();
    }
    function choose() {
      if (!mention || !matches[active]) return;
      const value = completeMention(input.value,mention,matches[active],projects);
      input.value = value.text; input.focus(); input.setSelectionRange(value.caret,value.caret);
      hidePicker(); preview();
    }
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
        ready = true; preview(); dismissed = false; updatePicker();
      } catch (e) { if (generation === sequence) { context.textContent = "Unable to load projects · close and retry"; fail(e.message); } }
    }
    function close() {
      if (saving) return;
      ++sequence; hidePicker(); dialog.close();
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
    input.oninput = () => { dismissed = false; preview(); updatePicker(); };
    input.addEventListener("click",() => { dismissed = false; updatePicker(); });
    input.addEventListener("keyup",event => {
      if (["ArrowLeft","ArrowRight","Home","End"].includes(event.key)) { dismissed = false; updatePicker(); }
    });
    input.addEventListener("select",updatePicker);
    input.addEventListener("blur",hidePicker);
    input.addEventListener("compositionstart",() => { composing = true; hidePicker(); });
    input.addEventListener("compositionend",() => { composing = false; dismissed = false; updatePicker(); });
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
      if (event.isComposing) return;
      if (event.target === input && !picker.hidden && !event.metaKey && !event.ctrlKey && !event.altKey) {
        if (["ArrowDown","ArrowUp"].includes(event.key) && matches.length) {
          event.preventDefault(); event.stopImmediatePropagation();
          active = (active + (event.key === "ArrowDown" ? 1 : -1) + matches.length) % matches.length; highlight(); return;
        }
        if (["Enter","Tab"].includes(event.key) && !event.shiftKey && matches.length) {
          event.preventDefault(); event.stopImmediatePropagation(); if (!event.repeat) choose(); return;
        }
        if (event.key === "Escape") {
          event.preventDefault(); event.stopImmediatePropagation(); dismissed = true; hidePicker(); return;
        }
      }
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
      const operation = {action:"create", title:parsed.title, body:"", labels:taskLabels([],kind.value), at_top:!bottom.checked};
      const key = JSON.stringify([host, parsed.project.id, operation]);
      if (pending?.key !== key) pending = {key, id:HeyBossUI.requestId()};
      hidePicker();
      saving = true; input.disabled = true; bottom.disabled = true; kind.disabled = true; submit.disabled = true; error.hidden = true; context.textContent = "Creating…";
      try {
        const value = await post(operation, parsed.project.name, pending.id);
        const savedHost = host;
        pending = null; input.value = ""; bottom.checked = false; kind.value = "implement"; saving = false; close();
        const link = document.createElement("a");
        link.href = `/#${new URLSearchParams({project:value.project.id, issue:value.issue.number, ...(savedHost ? {host:savedHost} : {})})}`;
        link.textContent = `Created #${value.issue.number} in ${value.project.name}`;
        status.replaceChildren(link); status.hidden = false;
        clearTimeout(statusTimer); statusTimer = setTimeout(() => status.hidden = true, 10000);
        window.dispatchEvent(new CustomEvent("hey-boss-issue-created", {detail:{...value, host:savedHost}}));
      } catch (e) { fail(`${e.message} Your title is preserved; retry to submit safely.`); }
      finally { saving = false; input.disabled = false; bottom.disabled = false; kind.disabled = false; if (dialog.open) { preview(); error.hidden = false; input.focus(); } }
    };
  }
  if (typeof document !== "undefined") init();
  return {parse, mentionAt, suggestions, completeMention, taskKind, taskLabels};
})();
if (typeof module !== "undefined") module.exports = HeyBossQuickIssue;
