"use strict";
const IssueFilters = (() => {
  const empty = {owner:"all", label:"", blocked:"", min_launches:"", search:""};
  function read(params) {
    const value = params.get("min_launches") || "";
    return {
      min_launches: /^[1-9]\d*$/.test(value) && Number.isSafeInteger(Number(value)) ? value : "",
      blocked: params.get("state") === "blocked" && ["hold", "dependencies"].includes(params.get("blocked")) ? params.get("blocked") : "",
    };
  }
  function matches(issue, route) {
    return (issue.agent_launch_count || 0) >= Number(route.min_launches || 0) && IssueBlockers.matches(issue, route.blocked);
  }
  function presets(route) {
    return [
      {key:"min_launches", value:"5", label:"5+ agent launches", icon:"refresh"},
      {key:"owner", value:"human:boss", label:"Assigned to boss", icon:"hat"},
      ...(route.state === "blocked" ? [{key:"blocked", value:"hold", label:"On hold", icon:"blocked"}] : []),
    ];
  }
  const selected = (route, option) => (option.key === "owner" && route.owner === "mine" ? "human:boss" : route[option.key]) === option.value;
  const toggle = (route, option) => ({[option.key]: selected(route, option) ? empty[option.key] : option.value});
  const active = route => Object.keys(empty).filter(key => key !== "search" && route[key] && route[key] !== empty[key]);
  function fields() {
    const owners = [...new Set(["human:boss", "watcher:github", ...model.assignmentMachines.map(m => `machine:${m.id}`), ...model.assignees,
      ...(!["all", "mine", "unassigned"].includes(model.route.owner) ? [model.route.owner] : [])])];
    return [
      {key:"owner", label:"Assignment", icon:"user", options:[
        {value:"all", label:"Any assignment"}, {value:"human:boss", label:"Assigned to boss", detail:model.boss.name},
        {value:"unassigned", label:"Unassigned"},
        ...owners.filter(id => id !== "human:boss").map(id => ({value:id, label:actorName(id), detail:id})),
      ]},
      {key:"label", label:"Label", icon:"tag", options:[{value:"", label:"Any label"},
        ...[...new Set([...model.labels, ...(model.route.label ? [model.route.label] : [])])].sort().map(value => ({value, label:value})),
      ]},
      {key:"min_launches", label:"Agent launches", icon:"refresh", options:[{value:"", label:"Any number of launches"},
        ...[...new Set(["1", "5", "10", ...(model.route.min_launches ? [model.route.min_launches] : [])])].sort((a,b) => a-b).map(value => ({value, label:`${value}+ agent launches`})),
      ]},
      ...(model.route.state === "blocked" ? [{key:"blocked", label:"Blocked reason", icon:"blocked", options:[
        {value:"", label:"Any blocked reason"}, {value:"hold", label:"On hold"}, {value:"dependencies", label:"Waiting for dependencies"},
      ]}] : []),
    ];
  }
  let category = "", trigger = null;
  function render() {
    const route = model.route;
    const chips = $("#active-issue-filters");
    const focusKey = chips.contains(document.activeElement) ? document.activeElement.dataset.editFilter || document.activeElement.dataset.removeFilter : null;
    const definitions = fields();
    const html = active(route).map(key => {
      const field = definitions.find(f => f.key === key);
      if (!field) return "";
      const value = key === "owner" && route.owner === "mine" ? "human:boss" : route[key];
      const name = key === "min_launches" ? `${value}+ launches` : field.options.find(o => o.value === value)?.label || value;
      return `<span class="issue-filter-chip"><button type="button" data-edit-filter="${key}" aria-label="Edit ${esc(field.label)} filter: ${esc(name)}" aria-haspopup="dialog">${icon(field.icon)}<span class="filter-chip-key">${esc(field.label)}</span><span class="filter-chip-value">${esc(name)}</span></button><button type="button" data-remove-filter="${key}" aria-label="Remove ${esc(field.label)} filter">${icon("x")}</button></span>`;
    }).join("");
    if (chips.innerHTML !== html) {
      chips.innerHTML = html;
      if (focusKey) ($(`[data-edit-filter="${focusKey}"]`, chips) || $("#add-issue-filter")).focus({preventScroll:true});
    }
    $("#clear-issue-filters").hidden = !active(route).length && !route.search;
    $("#applied-issue-filters").hidden = $("#clear-issue-filters").hidden;
    const quick = $("#issue-filter-presets");
    const options = presets(route);
    // Keep preset buttons in place during polling and selection, including focus.
    if (quick.dataset.state !== route.state) {
      quick.innerHTML = options.map(p => `<button type="button" class="issue-filter-preset" data-preset="${p.key}" aria-pressed="false">${icon(p.icon)}<span>${p.label}</span><span class="preset-check">${icon("check")}</span></button>`).join("");
      quick.dataset.state = route.state;
    }
    for (const option of options) $(`[data-preset="${option.key}"]`, quick).setAttribute("aria-pressed", String(selected(route, option)));
  }
  function position() {
    const menu = $("#issue-filter-menu");
    if (!menu.matches(":popover-open")) return;
    const bounds = (trigger?.isConnected ? trigger : $("#add-issue-filter")).getBoundingClientRect();
    const width = Math.min(344, innerWidth - 24);
    menu.style.width = width + "px";
    menu.style.left = Math.max(12, Math.min(bounds.left, innerWidth - width - 12)) + "px";
    const height = Math.min(menu.scrollHeight, 420, innerHeight - 24);
    const top = bounds.bottom + 8 + height <= innerHeight - 12 ? bounds.bottom + 8 : Math.max(12, bounds.top - height - 8);
    menu.style.top = top + "px";
    menu.style.maxHeight = Math.max(100, innerHeight - top - 12) + "px";
  }
  function choices() {
    const query = $("#issue-filter-search").value.trim().toLowerCase();
    const definitions = fields().filter(f => !category || f.key === category);
    $("#issue-filter-back").hidden = !category;
    $("#issue-filter-title").textContent = category ? definitions[0].label : "Add filter";
    $("#issue-filter-search").placeholder = category ? `Search ${definitions[0].label.toLowerCase()}…` : "Search filters…";
    const body = !category && !query
      ? definitions.map(f => `<button type="button" class="issue-filter-option" data-filter-category="${f.key}">${icon(f.icon)}<span>${f.label}</span>${icon("arrow-right")}</button>`).join("")
      : definitions.map(f => {
        const options = f.options.filter(o => `${f.label} ${o.label} ${o.detail || ""}`.toLowerCase().includes(query));
        if (!options.length) return "";
        return `${category ? "" : `<p class="filter-option-group">${f.label}</p>`}${options.map(o => `<button type="button" class="issue-filter-option" data-filter-key="${f.key}" data-filter-value="${esc(o.value)}" aria-pressed="${selected(model.route, {...o,key:f.key})}" title="${esc(o.detail || o.label)}">${icon(f.icon)}<span>${esc(o.label)}${o.detail && o.detail !== o.label ? `<small>${esc(o.detail)}</small>` : ""}</span><span class="filter-option-check">${icon("check")}</span></button>`).join("")}`;
      }).join("");
    $("#issue-filter-options").innerHTML = body || '<p class="filter-no-results" role="status">No matching filters</p>';
    position();
  }
  function close(restore = false) {
    const menu = $("#issue-filter-menu");
    if (menu.matches(":popover-open")) menu.hidePopover();
    if (restore) (trigger?.isConnected ? trigger : $("#add-issue-filter")).focus({preventScroll:true});
  }
  function open(button, key = "") {
    close();
    trigger = button;
    category = key;
    $("#issue-filter-search").value = "";
    choices();
    $("#issue-filter-menu").showPopover();
    button.setAttribute("aria-expanded", "true");
    position();
    $("#issue-filter-search").focus({preventScroll:true});
  }
  function apply(changes) {
    clearTimeout(searchTimer);
    return navigate({search:$("#issue-search").value, ...changes});
  }
  function init() {
    $("#add-issue-filter").onclick = e => open(e.currentTarget);
    $("#clear-issue-filters").onclick = () => { $("#add-issue-filter").focus(); apply(empty); };
    $("#active-issue-filters").onclick = e => {
      const edit = e.target.closest("[data-edit-filter]");
      const remove = e.target.closest("[data-remove-filter]");
      if (edit) open(edit, edit.dataset.editFilter);
      if (remove) apply({[remove.dataset.removeFilter]:empty[remove.dataset.removeFilter]});
    };
    $("#issue-filter-presets").onclick = e => {
      const button = e.target.closest("[data-preset]");
      if (button) apply(toggle(model.route, presets(model.route).find(p => p.key === button.dataset.preset)));
    };
    $("#issue-filter-options").onclick = e => {
      const button = e.target.closest("button");
      if (!button) return;
      if (button.dataset.filterCategory) {
        category = button.dataset.filterCategory;
        $("#issue-filter-search").value = "";
        choices();
        $("#issue-filter-search").focus();
      } else {
        close(true);
        apply({[button.dataset.filterKey]:button.dataset.filterValue});
      }
    };
    $("#issue-filter-back").onclick = () => { category = ""; $("#issue-filter-search").value = ""; choices(); $("#issue-filter-search").focus(); };
    $("#issue-filter-search").oninput = choices;
    $("#issue-filter-menu").addEventListener("beforetoggle", e => {
      if (e.newState === "closed") trigger?.setAttribute("aria-expanded", "false");
    });
    $("#issue-filter-menu").onkeydown = e => {
      if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(true); return; }
      const search = $("#issue-filter-search");
      const buttons = $$("#issue-filter-options button");
      if (e.key === "Enter" && e.target === search && buttons.length) { e.preventDefault(); buttons[0].click(); return; }
      if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key) || (!buttons.length)) return;
      if (e.target === search && ["Home", "End"].includes(e.key)) return;
      e.preventDefault();
      const index = buttons.indexOf(document.activeElement);
      const next = e.key === "Home" ? 0 : e.key === "End" ? buttons.length - 1 : index + (e.key === "ArrowDown" ? 1 : -1);
      if (next < 0) search.focus();
      else buttons[Math.min(buttons.length - 1, next)].focus();
    };
    window.addEventListener("resize", position);
    window.addEventListener("scroll", e => { if (!$("#issue-filter-menu").contains(e.target)) close(); }, true);
  }
  return {read, matches, presets, toggle, active, empty, render, init, close};
})();
