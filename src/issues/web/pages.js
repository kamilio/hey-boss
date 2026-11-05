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
  let csrf = '';
  const audience = value => ({public:'Public',organization:'Organization',private:'Private'}[value] || value);
  async function post(path, body) {
    if (!csrf) csrf = (await api('/api/bootstrap')).csrf;
    return api(path, {method:'POST', signal:AbortSignal.timeout(310000), headers:{'Content-Type':'application/json','x-hey-boss-csrf':csrf}, body:JSON.stringify(body)});
  }
  const dialog = node('dialog', null, 'pages-share-dialog');
  dialog.innerHTML = '<form><h2>Publish externally</h2><p class="muted" id="share-description"></p><label>Provider<select id="share-provider" required></select></label><p id="share-audience" class="muted"></p><label>Existing link or ID <span class="muted">(optional)</span><input id="share-link" autocomplete="off" placeholder="Link an existing publication"></label><p class="muted">An existing target will be updated with this page’s content.</p><label class="share-auto"><input id="share-auto" type="checkbox" checked>Auto-sync when this page is published again</label><p id="share-error" class="pages-error" role="alert" hidden></p><div class="dialog-actions"><button id="share-cancel" class="button" type="button">Cancel</button><button id="share-submit" class="button" type="submit">Publish</button></div></form>';
  dialog.setAttribute('aria-label', 'Publish externally'); document.body.append(dialog);
  let sharePage, shareRefresh, providers = [], shareSubmitting = false;
  dialog.addEventListener('cancel', event => { if (shareSubmitting) event.preventDefault(); });
  function providerChanged() {
    const name = $('share-provider').value, saved = (sharePage.publications || []).find(p=>p.provider===name);
    $('share-audience').textContent = `${audience(providers.find(p=>p.name===name)?.audience || '')} access · ${saved?.url ? 'Updates the saved link.' : 'Creates a link unless you supply one below.'}`;
    $('share-auto').checked = saved ? saved.auto_sync : true;
    $('share-submit').textContent = saved ? 'Save and sync' : 'Publish';
  }
  async function openShare(page, refresh) {
    sharePage = page; shareRefresh = refresh; $('share-error').hidden = true; $('share-link').value = '';
    $('share-description').textContent = `Share “${page.name}” through a configured provider.`;
    $('share-provider').replaceChildren(); $('share-submit').disabled = true;
    dialog.showModal();
    try {
      providers = (await api('/api/pages/providers')).providers;
      if (!providers.length) throw new Error('No publishing providers configured. Add them to ~/.hey-boss/publishing.yaml.');
      for (const provider of providers) { const option = node('option', `${provider.name} · ${audience(provider.audience)}`); option.value = provider.name; $('share-provider').append(option); }
      providerChanged(); $('share-submit').disabled = false; $('share-provider').focus();
    } catch(e) { $('share-error').textContent = e.message; $('share-error').hidden = false; }
  }
  $('share-provider').onchange = providerChanged;
  $('share-cancel').onclick = () => dialog.close();
  dialog.querySelector('form').onsubmit = async event => {
    event.preventDefault(); if (shareSubmitting) return; shareSubmitting = true;
    for (const control of dialog.querySelectorAll('button,input,select')) control.disabled = true;
    $('share-submit').textContent = 'Publishing…';
    $('share-error').hidden = true;
    try {
      await post('/api/pages/share', {name:sharePage.name, provider:$('share-provider').value, link:$('share-link').value.trim() || null, auto_sync:$('share-auto').checked});
      dialog.close(); await shareRefresh();
    } catch(e) { $('share-error').textContent = e.message; $('share-error').hidden = false; await shareRefresh(); }
    finally {
      shareSubmitting = false;
      for (const control of dialog.querySelectorAll('button,input,select')) control.disabled = false;
      $('share-submit').textContent = (sharePage.publications || []).some(p=>p.provider===$('share-provider').value) ? 'Save and sync' : 'Publish';
    }
  };
  function publishing(page, refresh) {
    const box = node('div', null, 'page-publishing');
    for (const publication of page.publications || []) {
      const row = node('div', null, 'page-publication');
      const link = node('a', `${publication.provider} · ${audience(publication.audience)}`);
      if (/^https?:\/\//.test(publication.url)) { link.href = publication.url; link.target = '_blank'; link.rel = 'noopener noreferrer'; }
      row.append(link, node('span', publication.error ? 'Sync failed' : publication.synced ? 'Synced' : 'Not synced', `page-state${publication.synced?'':' syncing'}`));
      const details = node('p', publication.auto_sync ? 'Auto-sync on publish' : 'Manual sync');
      if (publication.error) details.textContent += ` · ${publication.error}`;
      box.append(row, details);
    }
    const actions = node('div', null, 'page-publish-actions');
    const share = node('button', (page.publications || []).length ? 'Sharing settings' : 'Publish externally', 'button small');
    share.type = 'button'; share.onclick = () => openShare(page, refresh); share.disabled = page.syncing; actions.append(share);
    if ((page.publications || []).some(p=>!p.synced)) {
      const sync = node('button', 'Sync now', 'button small'); sync.type = 'button'; sync.disabled = page.syncing;
      sync.onclick = async () => { sync.disabled = true; sync.textContent = 'Syncing…'; try { await post('/api/pages/sync', {name:page.name}); await refresh(); } catch(e) { error(e); sync.disabled = false; sync.textContent = 'Sync now'; } };
      actions.append(sync);
    }
    box.append(actions); return box;
  }
  if ($('page-frame')) {
    const name = decodeURIComponent(location.pathname.slice('/pages/'.length));
    $('page-title').textContent = name; document.title = `${name} · Hey Boss`;
    $('page-frame').title = name;
    let contentURL = '', busy = false;
    async function load() {
      if (busy) return; busy = true;
      try {
        const {page, content_url} = await api(`/api/pages?name=${encodeURIComponent(name)}`);
        $('page-sharing').replaceChildren(publishing(page, load));
        $('pages-error').hidden = true;
        $('page-status').hidden = !page.syncing;
        $('page-status').textContent = page.deployment ? 'An update is syncing. Showing the last complete version.' : 'Files are syncing to this machine. The page will open when they arrive.';
        if (!content_url) { $('page-frame').hidden = true; $('page-frame').removeAttribute('src'); contentURL = ''; }
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
  let entries = [], next = null, selected = null, busy = false;
  const size = bytes => bytes < 1024 ? `${bytes} B` : bytes < 1048576 ? `${(bytes/1024).toFixed(1)} KiB` : `${(bytes/1048576).toFixed(1)} MiB`;
  function render() {
    $('pages-list').replaceChildren();
    for (const page of entries) {
      const card = node('article', null, 'page-card'), link = node('a', null, 'page-card-link'); link.href = page.path;
      const top = node('div', null, 'page-card-top'), mark = node('span', null, 'page-mark'); mark.append(icon('code'));
      top.append(mark, node('span', page.syncing ? 'Syncing' : 'Available here', `page-state${page.syncing?' syncing':''}`));
      const storage = node('div', null, 'page-storage');
      storage.append(node('strong', size(page.stored_size)), node('span', page.syncing ? 'files stored so far' : 'stored across versions'));
      storage.title = 'Compressed files on this machine, including retained versions. Shared files count toward each page.';
      link.append(top, node('h3', page.name), storage, node('p', `${page.files} ${page.files===1?'file':'files'} · ${size(page.size)} original`));
      const footer = node('div', null, 'page-card-footer'), info = node('span', `${page.machine} · ${new Date(page.updated_at).toLocaleDateString(undefined,{month:'short',day:'numeric'})}`);
      info.title = new Date(page.updated_at).toLocaleString();
      const remove = node('button', null, 'icon-button'); remove.type = 'button'; remove.setAttribute('aria-label', `Delete ${page.name}`); remove.title = 'Delete from all machines'; remove.append(icon('trash'));
      remove.onclick = () => { selected = page; $('pages-delete-description').textContent = `Delete “${page.name}” from all machines?${(page.publications || []).length ? ' External links remain hosted; delete them through their provider.' : ''}`; $('pages-delete-error').hidden = true; $('pages-delete-dialog').showModal(); $('pages-delete-cancel').focus(); };
      footer.append(info, remove); card.append(link, publishing(page, load), footer); $('pages-list').append(card);
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
  setInterval(() => { if (!document.hidden && !$('pages-delete-dialog').open && !dialog.open && next===null) load(); }, 10000);
})();
