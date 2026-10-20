(() => {
  const $ = id => document.getElementById(id);
  HeyBossUI.icons();
  let inventory = null, selected = null, filter = 'active';
  const node = (tag, text, cls) => { const el = document.createElement(tag); if (text != null) el.textContent = text; if (cls) el.className = cls; return el; };
  const machine = id => inventory.machines.find(m => m.machine.id === id)?.machine.label || id.slice(0, 12);
  const access = record => record.status === 'available';
  const date = ms => new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
  const state = r => ({ available: 'Granted', deleted: 'Deleted', not_granted: 'No grant', untrusted_writer: 'Untrusted owner' }[r.status] || 'Unavailable');
  function empty(title, text) { const el = node('div', null, 'vault-empty'); el.append(node('h3', title), node('p', text)); return el; }
  function detail(record) {
    const el = $('detail'); el.replaceChildren();
    el.append(node('span', record.kind.toUpperCase(), 'badge'), node('h2', record.name), node('p', `${state(record)} on this machine. Latest update from ${record.writer_label}.`, 'intro'));
    const dl = node('dl');
    for (const [label, value] of [['Updated', date(record.updated_ms)], ['Access owner', record.owner_label], ['Access version', String(record.access_version)], ['Revision', record.revision], ['Record ID', record.id]]) {
      const row = node('div'); row.append(node('dt', label), node('dd', value, label === 'Revision' || label === 'Record ID' ? 'fingerprint' : '')); dl.append(row);
    }
    el.append(dl, node('h3', 'Machines with access'));
    for (const id of record.recipients) {
      const card = node('div', null, 'machine-card'), label = node('div', machine(id));
      label.append(node('small', id.slice(0, 16), 'fingerprint')); card.append(label, node('span', id === record.owner ? 'Owner · read / write' : 'Read / write')); el.append(card);
    }
    el.append(node('p', 'Concurrent updates use the latest signed revision. Access changes are signed by the owner. Offline copies update when peers reconnect.', 'intro'));
  }
  function render() {
    if (!inventory) return;
    const q = $('search').value.trim().toLowerCase();
    const rows = inventory.records.filter(r => (filter === 'deleted' ? r.deleted : !r.deleted && (filter !== 'granted' || access(r))) && `${r.name} ${r.kind} ${r.writer_label} ${r.owner_label}`.toLowerCase().includes(q));
    $('records').replaceChildren();
    for (const record of rows) {
      const button = node('button', null, 'record'); button.type = 'button'; button.setAttribute('aria-pressed', String(selected === record.id));
      const body = node('span', null, 'record-body'); body.append(node('strong', record.name), node('small', `${record.kind} · ${record.recipients.length} machines`));
      button.append(node('span', record.kind === 'bundle' ? '{ }' : '●', 'record-icon'), body, node('span', state(record), 'record-status'));
      button.onclick = () => { selected = record.id; render(); detail(record); }; $('records').append(button);
    }
    if (!rows.length) $('records').append(empty(q ? 'No matches' : 'No records here yet', q ? 'Try another name or machine.' : inventory.machine ? 'Import a credential or save a named record with the CLI.' : 'Run hey-boss secrets init on this machine to get started.'));
    const active = inventory.records.filter(r => !r.deleted);
    $('total').textContent = active.length; $('granted').textContent = active.filter(access).length;
    $('storage').textContent = inventory.backend === 'keychain' ? 'Keychain' : inventory.backend === 'pass' ? 'pass' : inventory.backend;
    $('machine').textContent = inventory.machine?.machine.label || 'Not initialized';
    const current = inventory.records.find(r => r.id === selected); if (current) detail(current);
  }
  async function load() {
    $('refresh').disabled = true; $('error').hidden = true;
    try {
      const response = await fetch('/api/secrets', { cache: 'no-store' });
      if (!response.ok) throw new Error();
      inventory = await response.json(); render();
      $('status').textContent = `Local snapshot · ${new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;
    } catch {
      $('error').textContent = 'Inventory is unavailable. Check this machine’s local store and try again.'; $('error').hidden = false;
      $('status').textContent = inventory ? 'Showing the previous snapshot' : 'Inventory unavailable';
      if (!inventory) $('records').replaceChildren(empty('Could not load inventory', 'Refresh to try again.'));
    } finally { $('refresh').disabled = false; }
  }
  $('refresh').onclick = load; $('search').oninput = render;
  document.querySelectorAll('[data-filter]').forEach(button => button.onclick = () => {
    filter = button.dataset.filter;
    document.querySelectorAll('[data-filter]').forEach(b => b.setAttribute('aria-pressed', String(b === button))); render();
  });
  async function navigation() {
    const response = await fetch('/api/bootstrap', { cache: 'no-store', signal: AbortSignal.timeout(10000) });
    const boot = await response.json();
    if (!response.ok || !boot.ok) throw new Error('Could not load navigation');
    const projects = boot.projects || [];
    const initial = HeyBossUI.projectId(boot.project?.id || projects[0]?.id);
    const picker = new HeyBossUI.ProjectPicker({ onSelect(id) {
      // Project context only controls links back to project pages. Inventory is global.
      picker.update(projects, projects.find(p => p.id === id) || null);
    } });
    picker.update(projects, projects.find(p => p.id === initial) || boot.project || projects[0] || null);
  }
  navigation().catch(() => {
    $('connection').querySelector('span').textContent = 'Navigation unavailable';
  });
  load();
})();
