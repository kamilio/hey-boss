const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const test = require('node:test');

function fixture() {
  const requests = [], elements = new Map();
  const context = vm.createContext({
    document: {hidden: false},
    model: {route: {view: 'issues'}, detail: null, project: {id: 'fixture'}},
    Date,
    $: selector => {
      if (!elements.has(selector)) elements.set(selector, {hidden: false, setAttribute() {}});
      return elements.get(selector);
    },
    post: async (_url, action) => {
      requests.push(action.action);
      return action.action === 'count' ? {ok: true, unread: 4} : {ok: true, unread: 4, tasks: [{taskID: 'notice', status: 'pending'}]};
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

test('detail and inbox views retain complete notice lookup; hidden pages do not poll', async () => {
  const f = fixture();
  f.context.document.hidden = true;
  await f.run('refreshInboxBadge()');
  assert.equal(f.requests.length, 0);
  f.context.document.hidden = false;
  f.context.model.detail = {issue: {number: 7}};
  let related;
  f.context.loadRelatedNotices = async (...args) => { related = args; };
  await f.run('refreshInboxBadge()');
  assert.deepEqual(f.requests, ['list']);
  assert.equal(related[0], 7);
  assert.equal(related[3][0].taskID, 'notice');
  f.context.model.detail = null;
  f.context.model.route.view = 'inbox';
  await f.run('refreshInboxBadge()');
  assert.deepEqual(f.requests, ['list']);
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
