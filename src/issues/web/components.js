"use strict";
// Shared, dependency-free browser components. Page clients own data and mutations.
const HeyBossUI = (() => {
  // .test over HTTP is not a secure context. getRandomValues remains available;
  // randomUUID and SubtleCrypto do not. Keep retry IDs cryptographically random.
  function requestId() {
    if (crypto.randomUUID) return crypto.randomUUID();
    const bytes = crypto.getRandomValues(new Uint8Array(16));
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    const hex = [...bytes].map(byte => byte.toString(16).padStart(2, "0")).join("");
    return `${hex.slice(0,8)}-${hex.slice(8,12)}-${hex.slice(12,16)}-${hex.slice(16,20)}-${hex.slice(20)}`;
  }
  async function sha256(bytes) {
    if (crypto.subtle) {
      const digest = await crypto.subtle.digest("SHA-256", bytes);
      return [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, "0")).join("");
    }
    // FIPS 180-4 SHA-256 for stable mutation keys, including pending retries.
    // This fallback is not used for authentication or transport encryption.
    const constants = [
      0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
      0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
      0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
      0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
      0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
      0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
      0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
      0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
    ];
    const padded = new Uint8Array(Math.ceil((bytes.length + 9) / 64) * 64);
    padded.set(bytes);
    padded[bytes.length] = 128;
    const view = new DataView(padded.buffer);
    view.setUint32(padded.length - 8, Math.floor(bytes.length / 0x20000000));
    view.setUint32(padded.length - 4, (bytes.length * 8) >>> 0);
    const hash = new Uint32Array([0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19]);
    const words = new Uint32Array(64);
    const rotate = (value, count) => (value >>> count) | (value << (32 - count));
    for (let offset = 0; offset < padded.length; offset += 64) {
      for (let i = 0; i < 16; i++) words[i] = view.getUint32(offset + i * 4);
      for (let i = 16; i < 64; i++) {
        const x = words[i-15], y = words[i-2];
        const s0 = rotate(x,7) ^ rotate(x,18) ^ (x >>> 3);
        const s1 = rotate(y,17) ^ rotate(y,19) ^ (y >>> 10);
        words[i] = words[i-16] + s0 + words[i-7] + s1;
      }
      let [a,b,c,d,e,f,g,h] = hash;
      for (let i = 0; i < 64; i++) {
        const s1 = rotate(e,6) ^ rotate(e,11) ^ rotate(e,25);
        const choice = (e & f) ^ (~e & g);
        const t1 = (h + s1 + choice + constants[i] + words[i]) >>> 0;
        const s0 = rotate(a,2) ^ rotate(a,13) ^ rotate(a,22);
        const majority = (a & b) ^ (a & c) ^ (b & c);
        const t2 = (s0 + majority) >>> 0;
        h=g; g=f; f=e; e=(d+t1)>>>0; d=c; c=b; b=a; a=(t1+t2)>>>0;
      }
      const block = [a,b,c,d,e,f,g,h];
      for (let i = 0; i < 8; i++) hash[i] += block[i];
    }
    return [...hash].map(word => word.toString(16).padStart(8, "0")).join("");
  }
  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
  const esc = value => String(value ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[c]);
const paths = {
  bolt: '<path d="m13 2-9 12h7l-1 8 10-12h-7l1-8Z"/>',
  info: '<circle cx="12" cy="12" r="10" fill="currentColor" stroke="none"/><path d="M12 11v6M12 7v.5" stroke="var(--solid)" stroke-width="2"/>',
  success:
    '<circle cx="12" cy="12" r="10" fill="currentColor" stroke="none"/><path d="m7 12 3 3 7-7" stroke="var(--solid)" stroke-width="2"/>',
  warning:
    '<path d="M12 2 23 21H1Z" fill="currentColor" stroke="none"/><path d="M12 9v5M12 17v.5" stroke="var(--solid)" stroke-width="2"/>',
  error:
    '<path d="M8 2h8l6 6v8l-6 6H8l-6-6V8Z" fill="currentColor" stroke="none"/><path d="m8 8 8 8M16 8l-8 8" stroke="var(--solid)" stroke-width="2"/>',
  bell: '<path d="M5 17h14l-2-3V9a5 5 0 0 0-10 0v5l-2 3Z" fill="currentColor"/><path d="M10 21h4M12 2v2"/>',
  download: '<path d="M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5"/>',
  paperclip: '<path d="m21 11-8.5 8.5a6 6 0 0 1-8.5-8.5L13 2a4 4 0 0 1 5.7 5.7L9.5 17a2 2 0 0 1-2.8-2.8L15 6"/>',
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
  "pull-request": '<circle cx="6" cy="5" r="3"/><circle cx="6" cy="19" r="3"/><circle cx="18" cy="19" r="3"/><path d="M6 8v8M18 16V9a4 4 0 0 0-4-4h-2m3-3-3 3 3 3"/>',
  "pr-merged": '<circle cx="6" cy="5" r="3"/><circle cx="6" cy="19" r="3"/><circle cx="18" cy="19" r="3"/><path d="M6 8v8m0-8c0 5 12 3 12 8"/>',
  "pr-closed": '<circle cx="6" cy="5" r="3"/><circle cx="6" cy="19" r="3"/><path d="M6 8v8m8-10 6 6m0-6-6 6M17 16v6"/>',
  blocked: '<circle cx="12" cy="12" r="8.5"/><path d="M9 8v8M15 8v8"/>',
  closed:
    '<circle cx="12" cy="12" r="8.5"/><path d="m8.5 12 2.3 2.3 4.7-4.7"/>',
  check: '<path d="m5 12 4.5 4.5L19 7"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  copy: '<rect x="8" y="8" width="13" height="13" rx="2"/><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3"/>',
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
  archive: '<path d="M3 3h18v5H3ZM5 8v13h14V8M10 12h4"/>',
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

  // Share only the project ID across pages, including private HTTPS access.
  // Keep the existing key so desktop selections survive the upgrade.
  function projectId(fallback) {
    const explicit = new URLSearchParams(location.hash.slice(1)).get("project");
    if (explicit) return explicit;
    try {
      const saved = JSON.parse(localStorage.getItem("hey-boss-issues-project"));
      if (typeof saved === "string" && saved) return saved;
    } catch {}
    return fallback;
  }
  function projectNavigation(project) {
    const hash = new URLSearchParams({project});
    if ($("#nav-artifacts")) $("#nav-artifacts").href = `/artifacts#${hash}`;
    $("#nav-mindmaps").href = `/mm#${hash}`;
    $("#nav-workers").href = `/agents#${hash}`;
    if (location.pathname !== "/") {
      $("#nav-issues").href = `/#${hash}`;
      $("#nav-inbox").href = `/#${new URLSearchParams({project, view:"inbox"})}`;
    }
    $(".brand").href = `/#${hash}`;
    try { localStorage.setItem("hey-boss-issues-project", JSON.stringify(project)); } catch {}
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
        projectNavigation(project.id);
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
              `<div class="project-choice"><button class="project-option ${p.id === this.project?.id ? "selected" : ""}" data-project="${esc(p.id)}">${icon("folder")}<span class="project-option-info"><strong>${esc(p.name)}</strong><small class="project-activity">${p.activity_at ? `Active ${date(p.activity_at)}` : "No activity yet"}</small></span><span class="tab-count">${p.open ?? 0}</span>${p.id === this.project?.id ? `<span class="project-check">${icon("check")}</span>` : ""}</button>${this.onVisibility ? `<button class="icon-button project-visibility" data-project-visibility="${esc(p.id)}" aria-label="${p.hidden_at ? "Restore" : "Hide"} ${esc(p.name)}" title="${p.hidden_at ? "Restore project" : "Hide project"}">${icon(p.hidden_at ? "refresh" : "hide")}</button>` : ""}</div>`,
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
  return {requestId, sha256, icon, icons, relative, date, projectId, projectNavigation, ProjectPicker};
})();
