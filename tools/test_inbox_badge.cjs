const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const test = require('node:test');

function fixture() {
  const requests = [], elements = new Map();
  const context = vm.createContext({
    document: {hidden: false},
    model: {sequence: 0, route: {view: 'issues'}, detail: null, project: {id: 'fixture'}},
    Date,
    icon: () => '', esc: value => value, routeHash: route => '#notice=' + route.notice,
    $: selector => {
      if (!elements.has(selector)) elements.set(selector, {hidden: false, setAttribute() {}, contains() {return false}, querySelector() {return null}});
      return elements.get(selector);
    },
    post: async (_url, action) => {
      requests.push(action.action);
      return action.action === 'count' ? {ok: true, unread: 4} : {ok: true, unread: 4, tasks: [{taskID: 'notice', title: 'Linked notice', status: 'pending', issue: {project: 'fixture', number: 7}}]};
    },
  });
  vm.runInContext(fs.readFileSync(path.join(__dirname, '../src/issues/web/inbox.js'), 'utf8'), context);
  return {context, requests, elements, run: code => vm.runInContext(code, context)};
}

test('inactive inbox badge coalesces count requests without loading notice rows', async () => {
  const f = fixture();
  await Promise.all([f.run('refreshInboxBadge()'), f.run('refreshInboxBadge()')]);
  assert.deepEqual(f.requests, ['count']);
  assert.equal(f.elements.get('#inbox-unread').textContent, 4);
  assert.equal(f.run('inboxTasks.length'), 0);
  await f.run('inboxSnapshot()');
  assert.deepEqual(f.requests, ['count', 'list']);
  assert.equal(f.run('inboxTasks.length'), 1);
});

test('details request related notices; inbox retains complete lookup and hidden pages do not poll', async () => {
  const f = fixture();
  f.context.document.hidden = true;
  await f.run('refreshInboxBadge()');
  assert.equal(f.requests.length, 0);
  f.context.document.hidden = false;
  f.context.model.detail = {issue: {number: 7}};
  f.context.model.route.issue = 7;
  await f.run('refreshInboxBadge()');
  assert.deepEqual(f.requests, ['related']);
  assert.match(f.elements.get('#related-notices').innerHTML, /Linked notice/);
  assert.equal(f.run('inboxTasks.length'), 0);
  assert.equal(f.elements.get('#inbox-unread').textContent, 4);
  f.context.model.detail = null;
  f.context.model.route.view = 'inbox';
  await f.run('refreshInboxBadge()');
  assert.deepEqual(f.requests, ['related', 'list']);
});

test('related reads coalesce by full reference and discard replies after navigation', async () => {
  const f = fixture();
  f.context.model.detail = {issue: {number: 7}};
  Object.assign(f.context.model.route, {issue: 7, host: 'remote'});
  const pending = [];
  f.context.post = async (_url, action) => new Promise(resolve => pending.push({action, resolve}));
  const first = f.run('refreshInboxBadge()');
  const duplicate = f.run('loadRelatedNotices(7, "fixture", "remote")');
  assert.equal(pending.length, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(pending[0].action)), {action: 'related', issue: {project: 'fixture', number: 7, host: 'remote'}});
  f.context.model.route.host = '';
  ++f.context.model.sequence;
  const second = f.run('refreshInboxBadge()');
  assert.equal(pending.length, 2);
  pending[1].resolve({ok: true, unread: 3, tasks: []});
  await second;
  pending[0].resolve({ok: true, unread: 99, tasks: [{title: 'Stale notice'}]});
  await Promise.all([first, duplicate]);
  assert.equal(f.elements.get('#inbox-unread').textContent, 3);
  assert.doesNotMatch(f.elements.get('#related-notices').innerHTML, /Stale notice/);
});

test('failed count reads release their in-flight slot for the next refresh', async () => {
  const f = fixture();
  const post = f.context.post;
  f.context.post = async () => { throw Error('offline'); };
  await f.run('refreshInboxBadge()');
  assert.equal(f.elements.get('#inbox-unread').hidden, true);
  f.context.post = post;
  await f.run('refreshInboxBadge()');
  assert.deepEqual(f.requests, ['count']);
  assert.equal(f.elements.get('#inbox-unread').hidden, false);
});

test('a late count cannot overwrite the badge refreshed by opening Inbox', async () => {
  const f = fixture();
  const post = f.context.post;
  let resolve;
  f.context.post = async (url, action) => action.action === 'count'
    ? new Promise(done => { resolve = done; }) : post(url, action);
  const counting = f.run('refreshInboxBadge()');
  f.context.model.route.view = 'inbox';
  await f.run('inboxSnapshot()');
  resolve({ok: true, unread: 99});
  await counting;
  assert.equal(f.elements.get('#inbox-unread').textContent, 4);
});

test('a late count cannot overwrite a related-notice update after returning to the list', async () => {
  const f = fixture();
  const post = f.context.post;
  let resolve;
  f.context.post = async (url, action) => action.action === 'count'
    ? new Promise(done => { resolve = done; }) : post(url, action);
  const counting = f.run('refreshInboxBadge()');
  f.context.model.detail = {issue: {number: 7}};
  f.context.model.route.issue = 7;
  await f.run('refreshInboxBadge()');
  f.context.model.detail = null;
  f.context.model.route.issue = null;
  resolve({ok: true, unread: 99});
  await counting;
  assert.equal(f.elements.get('#inbox-unread').textContent, 4);
});

test('sender controls target the captured session and retain unknown historical trace', () => {
  const f = fixture();
  f.context.URLSearchParams = URLSearchParams;
  f.context.task = {taskID:'notice', issue:{project:'named:Trace'}, origin:{cwd:'/tmp/work',pid:123,agent:{id:'codex:exact',kind:'codex',machine:'m',host:'remote',session_id:'exact',creation_run:{id:'run/one',project_id:'named:Trace'},invocation:{offset:42}}}};
  assert.match(f.run('noticeAgentURL(task)'), /run=run%2Fone/);
  assert.match(f.run('noticeAgentURL(task)'), /at=42/);
  assert.match(f.run('noticeAgentURL(task, true)'), /steer=1/);
  assert.match(f.run('noticeSender(task)'), /Mute agent/);
  assert.match(f.run('noticeSender({...task,senderMuted:true})'), /Unmute agent/);
  assert.match(f.run('noticeSender({origin:{cwd:"old-checkout",pid:44}})'), /not recorded/);
  assert.match(f.run('noticeSender({origin:{cwd:"old-checkout",pid:44}})'), /old-checkout/);
});

test('sender shows the exact ID and model outside trace, with actions in a closed dropdown', () => {
  const f = fixture();
  f.context.URLSearchParams = URLSearchParams;
  f.context.task = {origin:{agent:{id:'codex:exact-session',kind:'codex',host:'devbox',model:'gpt-6-astra',session_id:'exact-session'}}};
  const html = f.run('noticeSender(task)');
  const summary = html.split('<summary>Sender trace</summary>')[0];
  assert.match(summary, /codex:exact-session/);
  assert.match(summary, /gpt-6-astra/);
  assert.match(html, /<details class="issue-overflow notice-agent-menu">/);
  assert.match(html, /aria-label="Agent actions"/);
  assert.doesNotMatch(html, /assignee-actions/);
  assert.match(f.run('noticeSender({origin:{agent:{id:"codex:older",kind:"codex",host:"devbox"}}})'), /Model not recorded/);
});

test('native mute destination opens the sender menu and focuses the exact notice mute action', () => {
  const f = fixture();
  Object.assign(f.context, {URLSearchParams, location:{hash:'#view=inbox&notice=exact-notice&sender=mute'}, secureLinks(){}, date(){return '';}});
  f.elements.set('#notice-sender', {scrollIntoView(){}});
  let focused = false;
  f.elements.set('[data-notice-mute]', {focus(){focused=true;}});
  f.run('renderNotice({taskID:"exact-notice",kind:"update",status:"pending",origin:{agent:{id:"codex:exact",model:"gpt-6-astra",host:"devbox"}}})');
  assert.equal(f.elements.get('.notice-agent-menu').open, true);
  assert.equal(focused, true);
  f.run('noticeAction = action => { capturedAction = action; }');
  f.elements.get('[data-notice-mute]').onclick();
  assert.deepEqual(JSON.parse(f.run('JSON.stringify(capturedAction)')), {action:'mute_agent',muted:true});
  assert.equal(f.run('inboxDetail.taskID'), 'exact-notice');
});
