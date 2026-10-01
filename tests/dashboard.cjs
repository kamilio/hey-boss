const {test} = require('node:test');
const assert = require('node:assert/strict');
// Exercise the actual overview script with a minimal DOM: upstream model labels
// are text, unknown usage is never rendered as zero, and stale status is visible.
test('Claude limits render unknown and stale readings safely', async () => {
  const vm = require('node:vm');
  const fs = require('node:fs');
  class Element {
    constructor(tag='div') { this.tag=tag; this.children=[]; this.listeners={}; this.hidden=false; this.value=''; this.dataset={}; this.textContent=''; this.classList={toggle(){}}; }
    append(...children) { this.children.push(...children); }
    replaceChildren(...children) { this.children=children; }
    addEventListener(name, fn) { this.listeners[name]=fn; }
    setAttribute(name, value) { this[name]=value; }
    removeAttribute(name) { delete this[name]; }
    get hash() { return this.href; }
  }
  const nodes = new Map();
  const html=fs.readFileSync(require.resolve('../src/proxy/overview.html'),'utf8');
  for(const match of html.matchAll(/id="([^"]+)"/g)) nodes.set(match[1],new Element());
  const catalog={mode:'standalone',relay:false,apis:[{id:'claude',name:'Claude subscription',description:'Native',configured:true,base_path:'',routes:[],models:[]}]};
  const payload={state:'stale',updated_at:1790800000,error:'Waiting before retrying',data:{windows:[{label:'<img src=x onerror=bad()>',used_percent:null,resets_at:null},{label:'Weekly',used_percent:112,resets_at:'bad-date'}],extra_usage:{enabled:false}}};
  let usageCalls=0;
  const context={document:{getElementById:id=>{assert.ok(nodes.has(id),`Missing element ${id}`);return nodes.get(id);},createElement:tag=>new Element(tag),documentElement:new Element(),addEventListener(){},hidden:false},location:{origin:'http://localhost',hash:''},window:{addEventListener(){}},setInterval(){},clearTimeout(){},setTimeout(){},localStorage:{setItem(){}},fetch:async path=>({ok:true,status:200,json:async()=>path==='/overview/api'?catalog:(usageCalls++,payload)})};
  vm.runInNewContext(fs.readFileSync(require.resolve('../src/proxy/overview.js'),'utf8'),context);
  await new Promise(resolve=>setImmediate(resolve));
  assert.equal(usageCalls,1);
  const cards=nodes.get('usage-windows').children;
  assert.equal(cards.length,2);
  assert.equal(cards[0].children[0].textContent,'<img src=x onerror=bad()>');
  assert.equal(cards[0].children.some(n=>n.tag==='progress'),false);
  assert.equal(cards[0].children[1].textContent,'Usage not reported');
  assert.equal(cards[1].children.find(n=>n.tag==='progress').value,100);
  assert.equal(cards[1].children.at(-1).textContent,'Reset time not reported');
  assert.match(nodes.get('usage-status').textContent,/Stale/);
  assert.equal(nodes.get('extra-usage').textContent,'Extra usage is disabled.');
  context.fetch=async()=>({ok:false,status:401});
  await nodes.get('refresh-usage').listeners.click();
  assert.match(nodes.get('usage-error').textContent,/session expired/);
  assert.match(nodes.get('usage-status').textContent,/stale/);
  assert.equal(nodes.get('refresh-usage').disabled,false);
});

function dashboard(fetcher) {
  const vm = require('node:vm');
  const fs = require('node:fs');
  const nodes = new Map();
  const html = fs.readFileSync(require.resolve('../src/proxy/logs.html'), 'utf8');
  for (const [, id] of html.matchAll(/id="([^"]+)"/g)) {
    nodes.set(id, {textContent: '', hidden: false, listeners: {}, addEventListener(name, fn) { this.listeners[name] = fn; }});
  }
  const intervals = [];
  const context = {
    document: {hidden: false, getElementById(id) { assert.ok(nodes.has(id), id); return nodes.get(id); }, addEventListener() {}},
    fetch: fetcher, URLSearchParams, AbortController, Intl, Date,
    setTimeout() { return 1; }, clearTimeout() {}, setInterval(fn) { intervals.push(fn); },
  };
  vm.runInNewContext(fs.readFileSync(require.resolve('../src/proxy/dashboard.js'), 'utf8'), context);
  return {nodes, context, refresh: () => nodes.get('refresh').listeners.click(), tick: () => intervals[0]()};
}
const settled = () => new Promise(resolve => setImmediate(resolve));
const totals = {snapshot_ms: Date.now(),
  today: {requests: 4, priced_requests: 3, unpriced_requests: 1, estimated_cost_usd: 1.25},
  week: {requests: 8, priced_requests: 0, unpriced_requests: 8, estimated_cost_usd: null},
  all_time: {requests: 20, priced_requests: 12, unpriced_requests: 8, estimated_cost_usd: 10.5}};

test('dashboard requests only bounded aggregates using local calendar boundaries', async () => {
  let requests = 0;
  const app = dashboard(async path => {
    requests++;
    const url = new URL(path, 'http://localhost');
    assert.equal(url.pathname, '/logs/api/dashboard');
    const day = new Date(Number(url.searchParams.get('day_start_ms')));
    const week = new Date(Number(url.searchParams.get('week_start_ms')));
    assert.equal(day.getHours(), 0);
    assert.equal(day.getMinutes(), 0);
    assert.equal(week.getHours(), 0);
    assert.equal(week.getDay(), 1);
    assert.ok(day >= week);
    return {ok: true, json: async () => ({rpm: 1234, source: 'host', spend: totals, logging: {enabled: true, status: 'healthy'}})};
  });
  await settled();
  assert.equal(requests, 1);
  assert.equal(app.nodes.get('today').textContent, '$1.25');
  assert.equal(app.nodes.get('week').textContent, 'Unavailable');
  assert.equal(app.nodes.get('all_time').textContent, '$10.50');
  assert.match(app.nodes.get('today-note').textContent, /partial estimate/);
  assert.match(app.nodes.get('scope').textContent, /Connected host/);
  assert.equal(app.nodes.get('status').textContent, 'Live');
  app.context.document.hidden = true;
  await app.tick();
  assert.equal(requests, 1);
});

test('dashboard keeps stale numbers, exposes gaps and never overlaps refreshes', async () => {
  let finish;
  const app = dashboard(() => new Promise(resolve => { finish = resolve; }));
  await app.refresh(); // Busy: must not start another fetch.
  finish({ok: true, json: async () => ({rpm: 3, spend: totals, logging: {enabled: true, status: 'gaps', dropped_events: 2}})});
  await settled();
  assert.match(app.nodes.get('status').textContent, /2 dropped/);
  app.context.fetch = async () => ({ok: false, status: 401});
  await app.refresh();
  assert.match(app.nodes.get('status').textContent, /Stale.*Session expired/);
  assert.equal(app.nodes.get('login').hidden, false);
  assert.equal(app.nodes.get('today').textContent, '$1.25');
  assert.equal(app.nodes.get('refresh').disabled, false);
  app.context.fetch = async () => ({ok: true, json: async () => ({rpm: 0, spend: null, logging: {enabled: false, status: 'disabled'}})});
  await app.refresh();
  assert.equal(app.nodes.get('today').textContent, 'Unavailable');
  assert.match(app.nodes.get('today-note').textContent, /disabled/);
  assert.equal(app.nodes.get('login').hidden, true);
});

test('dashboard clears recovered write errors while retaining real accounting gaps', async () => {
  let logging = {enabled: true, status: 'error', write_errors: 3, dropped_events: 0};
  const app = dashboard(async () => ({ok: true, json: async () => ({rpm: 3, spend: totals, logging})}));
  await settled();
  assert.equal(app.nodes.get('status').className, 'warning');
  assert.match(app.nodes.get('status').textContent, /Logging error/);

  // Retried writes all committed: the cumulative error counter remains nonzero.
  logging = {...logging, status: 'healthy', pending_events: 0};
  await app.refresh();
  assert.equal(app.nodes.get('status').textContent, 'Live');
  assert.equal(app.nodes.get('status').className, '');

  logging = {...logging, status: 'gaps', dropped_events: 2};
  await app.refresh();
  assert.equal(app.nodes.get('status').className, 'warning');
  assert.match(app.nodes.get('status').textContent, /2 dropped events/);
});
