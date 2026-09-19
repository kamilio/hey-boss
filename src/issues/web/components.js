"use strict";
// Shared, dependency-free browser components. Page clients own data and mutations.
const HeyBossUI = (() => {
  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
  const esc = value => String(value ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[c]);
const paths = {
  info: '<circle cx="12" cy="12" r="10" fill="currentColor" stroke="none"/><path d="M12 11v6M12 7v.5" stroke="var(--solid)" stroke-width="2"/>',
  success:
    '<circle cx="12" cy="12" r="10" fill="currentColor" stroke="none"/><path d="m7 12 3 3 7-7" stroke="var(--solid)" stroke-width="2"/>',
  warning:
    '<path d="M12 2 23 21H1Z" fill="currentColor" stroke="none"/><path d="M12 9v5M12 17v.5" stroke="var(--solid)" stroke-width="2"/>',
  error:
    '<path d="M8 2h8l6 6v8l-6 6H8l-6-6V8Z" fill="currentColor" stroke="none"/><path d="m8 8 8 8M16 8l-8 8" stroke="var(--solid)" stroke-width="2"/>',
  bell: '<path d="M5 17h14l-2-3V9a5 5 0 0 0-10 0v5l-2 3Z" fill="currentColor"/><path d="M10 21h4M12 2v2"/>',
  docs: '<path d="M5 2h9l5 5v15H5Z"/><path d="M14 2v6h5M8 12h8M8 16h8M8 19h5"/>',
  question:
    '<path d="M21 11a9 9 0 0 1-9 9H4l-3 3v-12a10 10 0 0 1 20 0Z"/><path d="M9 8a3 3 0 1 1 5 2l-2 2v1M12 16v.5"/>',
  build: '<path d="m14 3 7 7-3 3-3-3-9 11-3-3 11-9-3-3Z"/>',
  code: '<path d="m7 6-6 6 6 6M17 6l6 6-6 6M14 3l-4 18"/>',
  test: '<path d="m2 5 2 2 3-4M10 5h12M2 12l2 2 3-4M10 12h12M2 19l2 2 3-4M10 19h12"/>',
  review:
    '<path d="M13 3H3v18h7M6 7h7M6 11h4"/><circle cx="16" cy="14" r="5"/><path d="m20 18 3 4"/>',
  deploy:
    '<path d="m12 2 10 5v12l-10 4-10-4V7l10-5ZM2 7l10 5 10-5M12 12v11M7 4l10 5"/>',


  inbox: '<path d="M4 4h16l2 12v4H2v-4L4 4Z"/><path d="M2 16h6l2 3h4l2-3h6"/>',
  settings:
    '<circle cx="12" cy="12" r="3"/><path d="M10 2h4l.5 3.2L17 6.6l3-1.1L22 9l-2.5 2.1v2.8L22 16l-2 3.5-3-1.1-2.5 1.4L14 22h-4l-.5-2.2L7 18.4l-3 1.1L2 16l2.5-2.1v-2.8L2 9l2-3.5 3 1.1 2.5-1.4L10 2Z"/>',
  subtasks: '<path d="M6 3v14a3 3 0 0 0 3 3h3M6 8h6"/><rect x="12" y="5" width="8" height="6" rx="1.5"/><rect x="12" y="17" width="8" height="6" rx="1.5"/>',
  instructions:
    '<path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9l-6-6Z"/><path d="M14 3v6h6M8 13h8M8 17h5"/>',
  hide: '<path d="m3 3 18 18M10.6 10.6a2 2 0 0 0 2.8 2.8M9 5.5A11 11 0 0 1 12 5c6 0 10 7 10 7a18 18 0 0 1-4 4M6 6C3 8 2 12 2 12s4 7 10 7a12 12 0 0 0 5-1"/>',
  folder:
    '<path d="M3 7V5a2 2 0 0 1 2-2h4l2 3h8a2 2 0 0 1 2 2v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z"/>',
  chevrons: '<path d="m8 8 4-4 4 4M8 16l4 4 4-4"/>',
  search: '<circle cx="10.8" cy="10.8" r="7.3"/><path d="m16 16 4.5 4.5"/>',
  issue:
    '<circle cx="12" cy="12" r="8.5"/><circle cx="12" cy="12" r="2" fill="currentColor" stroke="none"/>',
  closed:
    '<circle cx="12" cy="12" r="8.5"/><path d="m8.5 12 2.3 2.3 4.7-4.7"/>',
  check: '<path d="m5 12 4.5 4.5L19 7"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  x: '<path d="m6 6 12 12M6 18 18 6"/>',
  tag: '<path d="M3 4h8l10 10-7 7L3 10V4Z"/><circle cx="7" cy="8" r="1"/>',
  user: '<circle cx="12" cy="8" r="3.5"/><path d="M5 21v-2a7 7 0 0 1 14 0v2"/>',
  refresh: '<path d="M20 8a8 8 0 1 0 .3 7M20 3v5h-5"/>',
  sync: '<path d="M20 8a8 8 0 0 0-14-3L3 8m0 0V3m0 5h5M4 16a8 8 0 0 0 14 3l3-3m0 0v5m0-5h-5"/>',
  grip: '<circle cx="9" cy="5" r="1"/><circle cx="15" cy="5" r="1"/><circle cx="9" cy="12" r="1"/><circle cx="15" cy="12" r="1"/><circle cx="9" cy="19" r="1"/><circle cx="15" cy="19" r="1"/>',
  sort: '<path d="M5 5h14M5 10h10M5 15h6M5 20h2"/>',
  comment:
    '<path d="M21 12a8.5 8.5 0 0 1-8.5 8.5 10 10 0 0 1-4-.8L3 21l1.4-5.2A8.5 8.5 0 1 1 21 12Z"/>',
  "arrow-right": '<path d="M4 12h16m-6-6 6 6-6 6"/>',
  "arrow-left": '<path d="M20 12H4m6-6-6 6 6 6"/>',
  trash: '<path d="M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7m4-7v7"/>',
  clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
  edit: '<path d="m15 4 5 5M4 20l1-6L17 2l5 5L10 19l-6 1Z"/>',
  spark:
    '<path d="m12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5L12 3Z"/>',
  link: '<path d="m10 13 4-4M8 16l-2 2a4 4 0 0 1-6-6l4-4a4 4 0 0 1 6 0M14 8l2-2a4 4 0 0 1 6 6l-4 4a4 4 0 0 1-6 0"/>',
};
const icon = (name) =>
  `<svg viewBox="0 0 24 24" aria-hidden="true">${paths[name] || paths.issue}</svg>`;
function icons(root = document) {
  $$("[data-icon]", root).forEach((el) => {
    el.innerHTML = icon(el.dataset.icon);
  });
}

function relative(at) {
  const delta = Math.max(0, Date.now() - at),
    m = Math.floor(delta / 60000);
  if (m < 1) return "just now";
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  return d < 30
    ? `${d}d ago`
    : new Date(at).toLocaleDateString(undefined, {
        month: "short",
        day: "numeric",
      });
}
function date(at) {
  return `<time datetime="${new Date(at).toISOString()}" title="${esc(new Date(at).toLocaleString())}">${relative(at)}</time>`;
}

  class ProjectPicker {
    constructor({onSelect, onVisibility}) {
      this.onSelect = onSelect; this.onVisibility = onVisibility;
      this.projects = []; this.project = null; this.showHiddenProjects = false;
      $("#project-trigger").onclick = () => {
        if (!$("#project-menu").hidden) { this.close(); return; }
        $("#project-menu").hidden = false;
        $("#project-trigger").setAttribute("aria-expanded", "true");
        $("#project-search").value = ""; this.render(); $("#project-search").focus();
      };
      $("#project-search").oninput = () => this.render();
      $("#toggle-hidden-projects").onclick = () => { this.showHiddenProjects = !this.showHiddenProjects; this.render(); };
      $("#project-options").onclick = event => {
        const visibility = event.target.closest("[data-project-visibility]");
        if (visibility) { this.onVisibility?.(visibility.dataset.projectVisibility); return; }
        const button = event.target.closest("[data-project]"); if (!button) return;
        this.close(); this.onSelect(button.dataset.project); $("#project-trigger").focus();
      };
      document.addEventListener("click", event => { if (!event.target.closest(".project-control")) this.close(); });
      $("#project-menu").addEventListener("keydown", event => {
        const controls = [$("#project-search"), ...$$("#project-options button"), $("#toggle-hidden-projects"), $("#add-project")].filter(el => el && !el.hidden && !el.disabled);
        const index = controls.indexOf(document.activeElement);
        if (["ArrowDown", "ArrowUp"].includes(event.key)) {
          event.preventDefault(); controls[(index + (event.key === "ArrowDown" ? 1 : controls.length - 1)) % controls.length].focus();
        }
        if (event.key === "Escape") { event.preventDefault(); this.close(); $("#project-trigger").focus(); }
      });
    }
    close() {
      const was = !$("#project-menu").hidden;
      $("#project-menu").hidden = true; $("#project-trigger").setAttribute("aria-expanded", "false"); return was;
    }
    update(projects, project) {
      this.projects = projects; this.project = project;
      $("#project-name").textContent = project?.name || "Projects";
      if (project) {
        const hash = new URLSearchParams({project:project.id});
        $("#nav-mindmaps").href = `/mm#${hash}`;
        if (location.pathname !== "/") $("#nav-issues").href = `/#${hash}`;
      }
      if (!$("#project-menu").hidden) this.render();
    }
    render() {
      const focused = document.activeElement;
      const focusProject = focused?.dataset.project;
      const focusVisibility = focused?.dataset.projectVisibility;
      const query = $("#project-search").value.toLocaleLowerCase();
      const hidden = this.projects.filter((p) => p.hidden_at).length;
      const matches = this.projects.filter(
        (p) =>
          Boolean(p.hidden_at) === this.showHiddenProjects &&
          (p.name + " " + p.id).toLocaleLowerCase().includes(query),
      );
      $("#project-sort-label").textContent = this.showHiddenProjects
        ? "HIDDEN PROJECTS"
        : "RECENT ACTIVITY";
      $("#toggle-hidden-projects").textContent = this.showHiddenProjects
        ? "Back to active projects"
        : `Hidden projects (${hidden})`;
      $("#toggle-hidden-projects").setAttribute(
        "aria-pressed",
        String(this.showHiddenProjects),
      );
      $("#project-options").innerHTML =
        matches
          .map(
            (p) =>
              `<div class="project-choice"><button class="project-option ${p.id === this.project?.id ? "selected" : ""}" data-project="${esc(p.id)}">${icon("folder")}<span class="project-option-info"><strong>${esc(p.name)}</strong><small>${esc(p.id.startsWith("local:") ? "Local directory" : p.id.replace(/^named:/, ""))}</small><small class="project-activity">${p.activity_at ? `Active ${date(p.activity_at)}` : "No activity yet"}</small></span><span class="tab-count">${p.open ?? 0}</span>${p.id === this.project?.id ? `<span class="project-check">${icon("check")}</span>` : ""}</button>${this.onVisibility ? `<button class="icon-button project-visibility" data-project-visibility="${esc(p.id)}" aria-label="${p.hidden_at ? "Restore" : "Hide"} ${esc(p.name)}" title="${p.hidden_at ? "Restore project" : "Hide project"}">${icon(p.hidden_at ? "refresh" : "hide")}</button>` : ""}</div>`,
          )
          .join("") ||
        `<div class="menu-empty">${query ? "No matching projects." : this.showHiddenProjects ? "No hidden projects." : "No active projects. Projects appear automatically when agents use them."}</div>`;
      if (focusProject || focusVisibility) {
        const target = $$("#project-options button").find((b) =>
          focusProject
            ? b.dataset.project === focusProject
            : b.dataset.projectVisibility === focusVisibility,
        );
        (target || $("#project-search")).focus();
      }
    }

  }
  return {icon, icons, relative, date, ProjectPicker};
})();
