/* Local, aggregate-only dashboard. No request archive is downloaded. */
(function () {
  'use strict';
  const $ = id => document.getElementById(id);
  const money = value => value == null ? 'Unavailable' : new Intl.NumberFormat('en-US', {
    style: 'currency', currency: 'USD', maximumFractionDigits: value > 0 && value < .01 ? 5 : 2,
  }).format(value);
  let busy = false;
  async function refresh() {
    if (busy || document.hidden) return;
    busy = true;
    $('refresh').disabled = true;
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 10000);
    try {
      const today = new Date();
      today.setHours(0, 0, 0, 0);
      const week = new Date(today);
      week.setDate(week.getDate() - (week.getDay() + 6) % 7);
      const query = new URLSearchParams({day_start_ms: today.getTime(), week_start_ms: week.getTime()});
      const response = await fetch(`/logs/api/dashboard?${query}`, {cache: 'no-store', signal: controller.signal});
      if (!response.ok) throw new Error(response.status === 401 ? 'Session expired. Sign in again.' : 'Dashboard unavailable. Retrying automatically.');
      const data = await response.json();
      $('scope').textContent = data.source === 'host' ? 'Connected host · client relays excluded' : 'This proxy · client relays excluded';
      $('rpm').textContent = new Intl.NumberFormat().format(data.rpm);
      for (const period of ['today', 'week', 'all_time']) {
        const value = data.spend?.[period];
        $(period).textContent = money(value?.estimated_cost_usd);
        $(period + '-note').textContent = value
          ? value.unpriced_requests
            ? `${value.priced_requests.toLocaleString()} priced / ${value.requests.toLocaleString()} requests · partial estimate`
            : value.requests ? 'All reported usage priced' : 'No requests yet'
          : data.logging.enabled ? 'Waiting for history' : 'Persistent logging is disabled';
      }
      const health = data.logging;
      // Retried writes can recover completely; only active failures or lost events imply gaps.
      const gaps = health.dropped_events || ['error', 'gaps', 'lagging'].includes(health.status);
      $('status').textContent = data.error || (gaps ? `Logging ${health.status}: ${health.dropped_events || 0} dropped events; estimates may be incomplete.`
        : health.status === 'initializing' ? 'Loading history…' : 'Live');
      $('status').className = data.error || gaps ? 'warning' : '';
      $('updated').textContent = data.spend ? `Spend updated ${new Date(data.spend.snapshot_ms).toLocaleTimeString()}` : '';
      $('login').hidden = true;
    } catch (error) {
      $('status').textContent = `Stale · ${error.name === 'AbortError' ? 'Refresh timed out. Retrying automatically.' : error.message}`;
      $('status').className = 'warning';
      $('login').hidden = !error.message.includes('Session expired');
    } finally {
      clearTimeout(timeout);
      busy = false;
      $('refresh').disabled = false;
    }
  }
  $('refresh').addEventListener('click', refresh);
  document.addEventListener('visibilitychange', refresh);
  setInterval(refresh, 5000);
  refresh();
})();
