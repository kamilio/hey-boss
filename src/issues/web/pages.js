(() => {
  const $ = id => document.getElementById(id);
  const node = (tag, text, cls) => { const e = document.createElement(tag); if (text != null) e.textContent = text; if (cls) e.className = cls; return e; };
  const icon = name => { const e = node('span'); e.innerHTML = HeyBossUI.icon(name); return e; };
  const error = e => { $('pages-error').textContent = e.message; $('pages-error').hidden = false; };
  async function api(url, options) {
    const response = await fetch(url, { cache:'no-store', signal:AbortSignal.timeout(15000), ...options });
    const value = await response.json();
    if (!response.ok || !value.ok) throw new Error(value.error?.message || 'Could not load published pages. Try again.');
    return value;
  }
  HeyBossUI.icons();
  if ($('page-frame')) {
    const name = decodeURIComponent(location.pathname.slice('/pages/'.length));
    $('page-title').textContent = name; document.title = `${name} · Hey Boss`;
    $('page-frame').title = name;
    let contentURL = '', busy = false;
    async function load() {
      if (busy) return; busy = true;
      try {
        const {page, content_url} = await api(`/api/pages?name=${encodeURIComponent(name)}`);
        $('pages-error').hidden = true;
        $('page-status').hidden = !page.syncing;
        $('page-status').textContent = page.deployment ? 'An update is syncing. Showing the last complete version.' : 'Files are syncing to this machine. The page will open when they arrive.';
        if (content_url && contentURL !== content_url) {
          contentURL = content_url;
          $('page-frame').src = content_url;
          $('page-frame').hidden = false;
        }
      } catch(e) { error(e); $('page-status').hidden = true; $('page-frame').hidden = true; $('page-frame').removeAttribute('src'); contentURL = ''; }
      finally { busy = false; }
    }
    $('page-copy').onclick = async () => {
      try { await navigator.clipboard.writeText(location.href); $('page-copy').lastElementChild.textContent = 'Copied'; setTimeout(() => $('page-copy').lastElementChild.textContent = 'Copy link', 1800); }
      catch { error(new Error('Copy the page URL from your address bar.')); }
    };
    load(); setInterval(() => { if (!document.hidden) load(); }, 10000);
    return;
  }
  let entries = [], next = null, csrf = '', selected = null, busy = false;
  const size = bytes => bytes < 1024 ? `${bytes} B` : bytes < 1048576 ? `${(bytes/1024).toFixed(1)} KB` : `${(bytes/1048576).toFixed(1)} MB`;
  function render() {
    $('pages-list').replaceChildren();
    for (const page of entries) {
      const card = node('article', null, 'page-card'), link = node('a', null, 'page-card-link'); link.href = page.path;
      const top = node('div', null, 'page-card-top'), mark = node('span', null, 'page-mark'); mark.append(icon('code'));
      top.append(mark, node('span', page.syncing ? 'Syncing' : 'Available here', `page-state${page.syncing?' syncing':''}`));
      link.append(top, node('h3', page.name), node('p', `${page.files} ${page.files===1?'file':'files'} · ${size(page.size)}`));
      const footer = node('div', null, 'page-card-footer'), info = node('span', `${page.machine} · ${new Date(page.updated_at).toLocaleDateString(undefined,{month:'short',day:'numeric'})}`);
      info.title = new Date(page.updated_at).toLocaleString();
      const remove = node('button', null, 'icon-button'); remove.type = 'button'; remove.setAttribute('aria-label', `Delete ${page.name}`); remove.title = 'Delete page'; remove.append(icon('trash'));
      remove.onclick = () => { selected = page; $('pages-delete-description').textContent = `Delete “${page.name}”?`; $('pages-delete-error').hidden = true; $('pages-delete-dialog').showModal(); $('pages-delete-cancel').focus(); };
      footer.append(info, remove); card.append(link, footer); $('pages-list').append(card);
    }
    $('pages-empty').hidden = entries.length > 0; $('pages-count').textContent = entries.length ? `${entries.length}${next===null?'':'+'}` : '';
    $('pages-more').hidden = next === null;
  }
  async function load(append = false) {
    if (busy) return; busy = true; $('pages-refresh').disabled = true; $('pages-more').disabled = true;
    try {
      const value = await api(`/api/pages?offset=${append?next:0}`);
      entries = append ? [...entries, ...value.pages.filter(p => !entries.some(old => old.name===p.name))] : value.pages; next = value.next_offset;
      render(); $('pages-error').hidden = true; $('pages-status').textContent = `${entries.filter(p=>p.syncing).length ? 'Files are syncing' : 'Up to date on this machine'}`;
    } catch(e) { error(e); $('pages-status').textContent = 'Could not refresh'; }
    finally { busy = false; $('pages-refresh').disabled = false; $('pages-more').disabled = false; }
  }
  $('pages-refresh').onclick = () => load(); $('pages-more').onclick = () => load(true);
  $('pages-delete-cancel').onclick = () => $('pages-delete-dialog').close();
  $('pages-delete-form').onsubmit = async event => {
    event.preventDefault(); if (!selected) return;
    $('pages-delete-submit').disabled = true;
    try {
      if (!csrf) csrf = (await api('/api/bootstrap')).csrf;
      await api('/api/pages/delete', { method:'POST', headers:{'Content-Type':'application/json','x-hey-boss-csrf':csrf}, body:JSON.stringify({name:selected.name}) });
      $('pages-delete-dialog').close(); selected = null; await load();
    } catch(e) { $('pages-delete-error').textContent = e.message; $('pages-delete-error').hidden = false; }
    finally { $('pages-delete-submit').disabled = false; }
  };
  api('/api/bootstrap').then(boot => {
    csrf = boot.csrf; const projects = boot.projects || [];
    const picker = new HeyBossUI.ProjectPicker({onSelect(id) { picker.update(projects, projects.find(p=>p.id===id)||null); }});
    picker.update(projects, projects.find(p=>p.id===HeyBossUI.projectId(boot.project?.id))||boot.project||null);
  }).catch(() => { $('connection').querySelector('span').textContent = 'Navigation unavailable'; });
  load();
  setInterval(() => { if (!document.hidden && !$('pages-delete-dialog').open && next===null) load(); }, 10000);
})();
