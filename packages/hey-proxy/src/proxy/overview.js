'use strict';
(() => {
  const $ = id => document.getElementById(id);
  let catalog, selected, timer;
  let usageBusy = false, usageAccount = null, usageGeneration = 0, accountsBusy = false;
  const node = (tag, text, className) => {
    const element = document.createElement(tag);
    if (text !== undefined) element.textContent = text;
    if (className) element.className = className;
    return element;
  };
  async function copy(text) {
    try {
      await navigator.clipboard.writeText(text);
      $('copy-status').textContent = 'Copied to clipboard';
    } catch {
      $('copy-status').textContent = 'Copy unavailable. Select the text to copy it.';
    }
    $('copy-status').hidden = false;
    clearTimeout(timer);
    timer = setTimeout(() => { $('copy-status').hidden = true; }, 2400);
  }
  function renderModels() {
    const query = $('search').value.trim().toLowerCase();
    const models = selected.models.filter(model => [model.id, ...model.routes.map(route => route.target)].some(text => text.toLowerCase().includes(query)));
    $('models').replaceChildren();
    $('model-count').textContent = query ? `${models.length} / ${selected.models.length}` : selected.models.length;
    for (const model of models) {
      const row = node('tr'), name = node('td'), destination = node('td');
      const button = node('button', model.id, 'model-button');
      button.type = 'button';
      button.title = `Copy model name: ${model.id}`;
      button.addEventListener('click', () => copy(model.id));
      name.append(button);
      for (const route of model.routes) {
        const mapping = node('div', undefined, 'mapping');
        mapping.append(route.target === model.id ? node('span', 'Direct', 'direct') : node('code', route.target));
        if (route.when) mapping.append(node('small', route.when));
        destination.append(mapping);
      }
      row.append(name, destination);
      $('models').append(row);
    }
    $('model-table').hidden = models.length === 0;
    $('empty').hidden = models.length !== 0;
    $('empty').textContent = query ? 'No configured models match your search.' : catalog.relay ? 'Model names are configured on the host.' : !selected.configured ? 'Configure a provider to use this API.' : selected.id === 'claude' ? 'Use the model selected in Claude Code. Native Claude requests pass through unchanged.' : 'No model names are listed for this API in the current config.';
  }
  function selectApi() {
    selected = catalog.apis.find(api => `#${api.id}` === location.hash) || catalog.apis.find(api => api.configured) || catalog.apis[0];
    for (const link of $('api-nav').children) {
      if (link.hash === `#${selected.id}`) link.setAttribute('aria-current', 'true');
      else link.removeAttribute('aria-current');
    }
    $('api-title').textContent = selected.name;
    $('description').textContent = selected.description;
    $('availability').textContent = catalog.relay ? 'Relayed to host' : selected.configured ? 'Provider configured' : 'Provider not configured';
    $('availability').classList.toggle('off', !selected.configured);
    $('base-url').textContent = location.origin + selected.base_path;
    $('routes').replaceChildren();
    for (const [method, path, note] of selected.routes) {
      const row = node('div', undefined, 'route');
      const resource = node('div');
      resource.append(node('code', path));
      if (note) resource.append(node('small', note, 'muted'));
      row.append(node('span', method, 'method'), resource);
      $('routes').append(row);
    }
    renderModels();
  }
  async function load() {
    $('refresh').disabled = true;
    $('error').hidden = true;
    try {
      const response = await fetch('/overview/api', {cache: 'no-store'});
      if (response.status === 401) {
        $('catalog').hidden = true;
        throw new Error('Your session expired. Reload the page to sign in.');
      }
      if (!response.ok) throw new Error(`Could not load config (HTTP ${response.status}).`);
      catalog = await response.json();
      $('mode').textContent = `${catalog.mode.charAt(0).toUpperCase() + catalog.mode.slice(1)} mode`;
      $('updated').textContent = `Config loaded ${new Date().toLocaleTimeString([], {hour:'2-digit', minute:'2-digit'})}`;
      $('api-nav').replaceChildren();
      for (const api of catalog.apis) {
        const link = node('a');
        link.href = `#${api.id}`;
        link.append(node('span', api.name), node('small', catalog.relay ? 'relay' : api.models.length));
        $('api-nav').append(link);
      }
      $('relay-note').hidden = !catalog.relay;
      $('catalog').hidden = false;
      selectApi();
      $('claude-usage').hidden = !usageAccount && !catalog.relay && !catalog.apis.some(api => api.id === 'claude' && api.configured);
      if (!$('claude-usage').hidden) loadUsage();
    } catch (error) {
      $('error').textContent = error.message;
      $('error').hidden = false;
      $('updated').textContent = catalog ? 'Refresh failed · showing previous config' : 'Config unavailable';
    } finally {
      $('refresh').disabled = false;
    }
  }
  async function loadAccounts() {
    if (accountsBusy) return;
    accountsBusy = true;
    $('refresh-accounts').disabled = true;
    try {
      const response = await fetch('/providers/v1', {cache: 'no-store'});
      if (!response.ok) throw new Error(response.status === 404 ? 'Named accounts require an updated proxy host.' : 'Account status unavailable. Refresh to try again.');
      const data = await response.json();
      if (data.schema_version !== 1 || !Array.isArray(data.connections)) throw new Error('Update this client to read the host’s account format.');
      $('connections').replaceChildren();
      const counts = new Map();
      for (const account of data.connections) if (account.account_ref) counts.set(account.account_ref, (counts.get(account.account_ref) || 0) + 1);
      for (const account of data.connections) {
        const card = node('article', undefined, 'connection-card');
        card.append(node('h3', account.name));
        const meta = node('div', undefined, 'connection-meta');
        meta.append(node('span', account.implementation === 'openai' ? 'OpenAI compatible' : account.implementation === 'codex' ? 'Codex' : 'Claude'));
        meta.append(node('span', account.auth === 'subscription' ? 'Subscription' : 'API billing'));
        meta.append(node('span', account.ready ? 'Ready' : 'Needs sign-in or credentials', account.ready ? 'badge' : 'badge off'));
        card.append(meta);
        if (counts.get(account.account_ref) > 1) card.append(node('p', 'Shared subscription · limits also apply to its other aliases.', 'note'));
        const actions = node('div', undefined, 'connection-actions');
        if (account.auth === 'subscription') {
          const limits = node('button', 'View limits'); limits.type = 'button'; limits.disabled = !account.ready;
          limits.setAttribute('aria-label', 'View limits for ' + account.name);
          limits.addEventListener('click', () => {
            usageAccount = account; usageGeneration++; usageBusy = false;
            $('claude-usage').hidden = false;
            $('claude-usage-title').textContent = account.name + ' · subscription limits';
            $('usage-windows').replaceChildren(); $('extra-usage').hidden = true; $('usage-error').hidden = true;
            $('usage-status').textContent = 'Loading limits…';
            loadUsage();
            $('claude-usage').scrollIntoView({behavior:'smooth', block:'start'});
          });
          actions.append(limits);
        }
        const connection = node('button', 'Copy connection'); connection.type = 'button'; connection.disabled = !account.ready;
        connection.setAttribute('aria-label', 'Copy connection for ' + account.name);
        connection.addEventListener('click', () => copy(JSON.stringify({provider:account.name, account_ref:account.account_ref})));
        actions.append(connection); card.append(actions); $('connections').append(card);
      }
      $('connection-status').textContent = data.connections.length ? data.connections.length + ' connections · credentials stay on the host.' : 'No named accounts configured. Default connections remain available below.';
    } catch (error) {
      $('connections').replaceChildren();
      $('connection-status').textContent = error.message;
    } finally { accountsBusy = false; $('refresh-accounts').disabled = false; }
  }
  $('refresh-accounts').addEventListener('click', loadAccounts);
  loadAccounts();
  async function loadUsage() {
    if (usageBusy || document.hidden || $('claude-usage').hidden) return;
    usageBusy = true;
    const generation = usageGeneration;
    $('refresh-usage').disabled = true;
    try {
      const response = await fetch(usageAccount ? '/usage/v1/' + encodeURIComponent(usageAccount.implementation) + '/' + encodeURIComponent(usageAccount.name) : '/claude/usage', {cache: 'no-store'});
      if (response.status === 401) throw new Error('Your session expired. Reload the page to sign in.');
      if (!response.ok) throw new Error(`Could not load limits (HTTP ${response.status}).`);
      const usage = await response.json();
      if (generation !== usageGeneration) return;
      $('usage-windows').replaceChildren();
      $('extra-usage').hidden = true;
      $('usage-error').hidden = !usage.error;
      $('usage-error').textContent = usage.error || '';
      if (usage.state === 'disabled') {
        $('usage-status').textContent = 'Claude is not configured on this proxy host.';
        return;
      }
      const fetched = usage.updated_at ? new Date(usage.updated_at * 1000).toLocaleString() : null;
      $('usage-status').textContent = fetched ? `${usage.state === 'stale' ? 'Stale · last updated' : 'Updated'} ${fetched}` : 'No subscription reading available yet. Check this account’s sign-in on the proxy host.';
      for (const window of usage.data?.windows || []) {
        const card = node('article', undefined, 'usage-card');
        card.append(node('h3', window.label));
        const used = window.used_percent;
        if (typeof used === 'number' && Number.isFinite(used)) {
          card.append(node('strong', `${used.toLocaleString(undefined, {maximumFractionDigits:1})}% used`));
          if (typeof window.remaining_percent === 'number' && Number.isFinite(window.remaining_percent)) {
            card.append(node('p', `${window.remaining_percent.toLocaleString(undefined, {maximumFractionDigits:1})}% left`));
          }
          const bar = node('progress');
          bar.max = 100; bar.value = Math.min(100, Math.max(0, used));
          bar.setAttribute('aria-label', `${window.label}: ${used}% used`);
          card.append(bar);
          card.classList.toggle('full', used >= 90);
        } else card.append(node('p', 'Usage not reported'));
        const reset = window.resets_at ? new Date(window.resets_at) : null;
        card.append(node('p', reset && Number.isFinite(reset.getTime()) ? `Resets ${reset.toLocaleString()}` : 'Reset time not reported'));
        $('usage-windows').append(card);
      }
      const extra = usage.data?.extra_usage;
      if (extra) {
        $('extra-usage').hidden = false;
        $('extra-usage').textContent = extra.enabled === false ? 'Extra usage is disabled.' : `Extra usage: ${extra.enabled === true ? 'enabled' : 'status not reported'}${typeof extra.used_percent === 'number' ? ` · ${extra.used_percent.toLocaleString(undefined, {maximumFractionDigits:1})}% used` : ''}.`;
        if (extra.spend) {
          const spend = extra.spend;
          const money = value => typeof value === 'number' && Number.isFinite(value) && value >= 0
            ? `${spend.currency} ${value.toLocaleString(undefined, {minimumFractionDigits:2, maximumFractionDigits:2})}` : 'unknown';
          $('extra-usage').textContent += ` Provider-reported monthly extra spend: ${money(spend.used)} · cap: ${money(spend.limit)} · remaining: ${money(spend.remaining)} · above cap: ${money(spend.over_limit)}. This is separate from estimated API-equivalent spend.`;
        }
      }
    } catch (error) {
      if (generation !== usageGeneration) return;
      $('usage-error').hidden = false;
      $('usage-error').textContent = error.message;
      $('usage-status').textContent = 'Refresh failed · any displayed limits are stale.';
    } finally {
      if (generation !== usageGeneration) return;
      usageBusy = false;
      $('refresh-usage').disabled = false;
    }
  }
  $('refresh-usage').addEventListener('click', loadUsage);
  setInterval(loadUsage, 60000);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) loadUsage(); });
  $('origin').textContent = location.origin;
  $('refresh').addEventListener('click', load);
  $('search').addEventListener('input', () => { if (selected) renderModels(); });
  $('copy-base').addEventListener('click', () => copy($('base-url').textContent));
  $('theme').addEventListener('click', () => {
    const theme = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
    document.documentElement.dataset.theme = theme;
    try { localStorage.setItem('hey-proxy-theme', theme); } catch {}
  });
  window.addEventListener('hashchange', () => { if (catalog) selectApi(); });
  load();
})();
