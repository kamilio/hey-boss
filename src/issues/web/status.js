/* Read-only progress, shared by desktop and paired-device issue viewers. */
window.HeyBossStatus = (() => {
  const labels = {green: "On track", orange: "At risk", red: "In trouble"};
  const esc = value => String(value ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[c]);
  const label = status => labels[status?.level] || "No update";
  const badge = status => `<span class="progress-badge progress-${esc(status.level)}"><span class="progress-dot" aria-hidden="true"></span>${label(status)}</span>`;
  function list(issue) {
    const s = issue.status;
    if (!s) return "";
    const previous = issue.assignee && issue.assignee !== s.author ? " · Previous owner" : "";
    const help = `${label(s)} · ${s.comment}${previous} · ${new Date(s.created_at).toLocaleString()}`;
    return `<span class="issue-progress progress-${esc(s.level)}" role="img" tabindex="0" aria-label="${esc(help)}"><span class="progress-dot" aria-hidden="true"></span><span class="issue-progress-help" aria-hidden="true">${esc(help)}</span></span>`;
  }
  function positionHelp(dot) {
    const help = dot.querySelector('.issue-progress-help');
    dot.classList.remove('progress-help-below');
    const bounds = dot.getBoundingClientRect();
    help.style.left = `${Math.max(16, Math.min(bounds.left, innerWidth - help.offsetWidth - 16)) - bounds.left}px`;
    const panel = dot.closest('.issue-panel');
    if (help.getBoundingClientRect().top < Math.max(16, panel?.getBoundingClientRect().top || 0)) dot.classList.add('progress-help-below');
  }
  // Delegation also handles refreshed lists without adding per-row listeners.
  for (const type of ['pointerover', 'focusin']) document.addEventListener(type, event => {
    const dot = event.target.closest('.issue-progress');
    if (dot && !dot.contains(event.relatedTarget)) {
      dot.classList.remove('progress-help-dismissed');
      positionHelp(dot);
    }
  });
  window.addEventListener('resize', () => document.querySelectorAll('.issue-progress:hover, .issue-progress:focus').forEach(positionHelp));
  document.addEventListener('keydown', event => {
    if (event.key !== 'Escape') return;
    const dots = document.querySelectorAll('.issue-progress:hover, .issue-progress:focus');
    if (!dots.length) return;
    dots.forEach(dot => dot.classList.add('progress-help-dismissed'));
    event.preventDefault();event.stopImmediatePropagation();
  }, true);
  function current(issue, name) {
    const s = issue.status;
    if (!s) return '<div class="progress-empty"><span class="progress-dot" aria-hidden="true"></span>No status update yet</div>';
    return `<div class="progress-heading">${badge(s)}<span class="progress-caption">Progress</span></div><p class="progress-comment">${esc(s.comment)}</p><div class="progress-meta"><span title="${esc(s.author)}">${esc(name(s.author))}${issue.assignee && issue.assignee !== s.author ? " · Previous owner" : ""}</span><span>Updated ${HeyBossUI.date(s.created_at)}</span></div>`;
  }
  function card(issue, name) {
    return `<section class="issue-progress-card" aria-label="Issue progress"><div class="progress-current">${current(issue, name)}</div>${issue.status ? `<details class="progress-history"><summary>${HeyBossUI.icon("clock")}Status history</summary><div class="progress-history-content"><p class="progress-history-note">Progress updates, newest first. Comments keep lasting findings.</p><ol class="progress-history-list" tabindex="0" aria-label="Status updates"></ol><p class="progress-history-error" role="alert" hidden></p><div class="progress-history-actions"><button class="progress-history-refresh" type="button" hidden>Refresh history</button><button class="progress-history-more" type="button" hidden>Show older updates</button></div></div></details>` : ""}</section>`;
  }
  function mount(root, issue, name, read) {
    let latest = issue, offset = 0, snapshotAt = null, generation = 0, loaded = false, busy = false;
    const details = root.querySelector("details"), list = root.querySelector("ol");
    const more = root.querySelector(".progress-history-more"), refresh = root.querySelector(".progress-history-refresh"), error = root.querySelector(".progress-history-error");
    async function load(reset = false) {
      if (busy || !details) return;
      busy = true;const ticket = ++generation;
      const focusList = document.activeElement === more || document.activeElement === refresh;
      more.disabled = refresh.disabled = true;error.hidden = true;
      list.setAttribute("aria-busy", "true");
      if (!loaded) list.innerHTML = '<li class="progress-history-loading" role="status">Loading updates…</li>';
      try {
        const result = await read(reset ? 0 : offset, reset ? null : snapshotAt);
        if (!root.isConnected || ticket !== generation) return;
        const html = result.updates.map(s => `<li><div class="progress-history-heading">${badge(s)}${HeyBossUI.date(s.created_at)}</div><p>${esc(s.comment)}</p><span class="progress-history-author" title="${esc(s.author)}">${esc(name(s.author))}</span></li>`).join("");
        if (reset || !loaded) {
          list.innerHTML = html || '<li class="progress-history-loading">No status updates yet.</li>';
          list.scrollTop = 0;
        } else {
          const previousCount = list.children.length;
          list.insertAdjacentHTML("beforeend", html);
          const firstOlder = list.children[previousCount];
          if (firstOlder) list.scrollTop += firstOlder.getBoundingClientRect().top - list.getBoundingClientRect().top;
        }
        if (focusList) list.focus({preventScroll:true});
        loaded = true;offset = result.next_offset;snapshotAt = result.snapshot_at ?? null;more.hidden = offset == null;
        refresh.hidden = !(latest.status?.created_at > snapshotAt);refresh.textContent = "Refresh history";
      } catch (e) {
        if (!root.isConnected || ticket !== generation) return;
        if (!loaded) list.replaceChildren();
        error.textContent = e.message || "Could not load status history. Try again.";error.hidden = false;
        refresh.hidden = false;refresh.textContent = "Retry history";
      } finally {
        busy = false;more.disabled = refresh.disabled = false;list.removeAttribute("aria-busy");
      }
    }
    if (details) {
      list.addEventListener("keydown", event => {
        if (event.target !== list || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
        const positions = {ArrowDown:list.scrollTop+40, ArrowUp:list.scrollTop-40, PageDown:list.scrollTop+list.clientHeight*.9, PageUp:list.scrollTop-list.clientHeight*.9, Home:0, End:list.scrollHeight};
        if (!(event.key in positions)) return;
        event.preventDefault();list.scrollTop = positions[event.key];
      });
      details.addEventListener("toggle", () => {if (details.open && !loaded) load(true);});
      more.onclick = () => load();refresh.onclick = () => load(true);
    }
    return next => {
      if (JSON.stringify(latest.status) === JSON.stringify(next.status) && latest.assignee === next.assignee) return;
      // The first update adds the history disclosure without touching the editor.
      if (!latest.status && next.status) {
        root.outerHTML = card(next, name);
        return "remount";
      }
      latest = next;
      root.querySelector(".progress-current").innerHTML = current(next, name);
      if (refresh && loaded) {refresh.hidden = false;refresh.textContent = "Refresh history";}
    };
  }
  return {list, card, mount};
})();
