// Isolated issue design review. No real agents, issue data, or fleet services.
import {mkdirSync, rmSync, copyFileSync, readFileSync} from 'node:fs';
import {spawn, spawnSync} from 'node:child_process';
import {createServer, request} from 'node:http';
import {resolve} from 'node:path';
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';

const root = resolve('output/playwright/issue121');
mkdirSync(root, {recursive: true});
for (const suffix of ['', '-wal', '-shm']) rmSync(root + '/issues.db' + suffix, {force: true});
const binary = root + '/hey-boss-visual';
copyFileSync(resolve('target/debug/hey-boss'), binary);
const project = {id: 'named:Issue Design QA', name: 'Issue Design QA'};
const env = {...process.env, HEY_BOSS_ISSUE_DB: root + '/issues.db',
  HEY_BOSS_FLEET_STATE: root, HEY_BOSS_INBOX_SOCKET: root + '/inbox.sock'};
for (const key of ['HEY_BOSS_ISSUE_HOST', 'HEY_BOSS_ISSUE_PROJECT', 'HEY_BOSS_AGENT_ID']) delete env[key];
const cli = (args, kind = 'issue') => {
  const result = spawnSync(binary, [kind, '--project', project.id, '--agent', 'human:qa', '--json', ...args], {env, encoding: 'utf8'});
  if (result.status) throw Error(result.stdout + result.stderr);
  return JSON.parse(result.stdout);
};
for (const [title, body] of [
  ['Reconnect reliably after waking from sleep', '## Expected behavior\n\nReconnect without losing the conversation.\n\n- [ ] Restore the connection\n- [x] Preserve the draft\n\nKeep findings in the discussion below.'],
  ['An issue ready for the next agent', ''],
  ['Explore the next release', 'Refine the scope before starting work.'],
  ['Waiting for the reconnect fix', 'Resume when the blocking work is complete.'],
  ['Completed design review', 'The design and keyboard controls have been reviewed.'],
  ['A long issue title that remains understandable and readable on even the narrowest supported phone screen', 'A long description with an identifier: ' + 'reconnect-'.repeat(60)],
]) cli(['create', '--title', title, '--body', body, '--label', 'enhancement']);
cli(['claim', '1']);
for (let n = 0; n < 22; n++) cli(['status', '1', 'green', '--comment', `Checking reconnect behavior, step ${n + 1}.`]);
cli(['status', '1', 'green', '--comment', 'The fix passes tests. Checking the phone layout next.']);
cli(['comment', '1', '--body', 'The conversation and draft now survive reconnecting.']);
cli(['pr', 'add', '1', 'https://github.com/example/hey-boss/pull/42', '--purpose', 'fix']);
cli(['create', '--title', 'Preserve the unsent draft', '--body', 'Keep text across reconnects.']);
cli(['subtask', 'add', '1', '7']);
cli(['edit', '3', '--draft']);
cli(['block', '4', '--by', '2', '--comment', 'Waiting for the reconnect fix.']);
cli(['close', '5', '--comment', 'Reviewed and complete.']);
cli(['claim', '6']);
cli(['create', '--title', 'Archived issue']);
cli(['delete', '8']);
cli(['create', '--title', 'Implementation notes', '--body', '## Findings\n\nThe reconnect behavior is now reliable.', '--issue', '1'], 'artifact');

const web = spawn(binary, ['issue', '--project', project.id, 'web', '--port', '48121', '--no-discovery', '--json'], {env, stdio: ['ignore', 'inherit', 'inherit']});
// Read current assets so design iteration does not wait on unrelated Rust builds.
const proxy = createServer((req, res) => {
  const file = req.url.split('?')[0];
  if (['/app.js', '/app.css'].includes(file)) {
    res.setHeader('Content-Type', file.endsWith('.js') ? 'application/javascript' : 'text/css');
    res.end(readFileSync(resolve('src/issues/web/' + file.slice(1))));
    return;
  }
  const headers = {...req.headers, host: '127.0.0.1:48121'};
  // Ports differ only inside this isolated fixture; browsers call them same-site.
  delete headers['sec-fetch-site'];
  for (const key of ['origin', 'referer']) if (headers[key]) headers[key] = headers[key].replace('127.0.0.1:48122', '127.0.0.1:48121');
  const upstream = request({hostname: '127.0.0.1', port: 48121, path: req.url, method: req.method, headers}, reply => {
    res.writeHead(reply.statusCode, reply.headers); reply.pipe(res);
  });
  upstream.on('error', () => {res.writeHead(502); res.end('Fixture starting');});
  req.pipe(upstream);
});
proxy.listen(48122, '127.0.0.1');
const store = new HubStore(); store.setIssueProjects([project]);
const hubToken = 'synthetic-design-fixture-'.repeat(4);
const app = createApp({store, hubToken, secure: false, origin: 'http://127.0.0.1:52121'});
app.get('/fixture/pairing', (_req, res) => res.json({code: store.pairing()}));
const mobile = app.listen(52121, '127.0.0.1');
let pumping = false;
const timer = setInterval(async () => {
  if (pumping) return;
  pumping = true;
  const hub = 'http://127.0.0.1:52121', desktop = 'http://127.0.0.1:48121';
  const headers = {Authorization: 'Bearer ' + hubToken, 'Content-Type': 'application/json'};
  try {
    const {requests} = await (await fetch(hub + '/api/bridge/web', {headers})).json();
    await Promise.all(requests.map(async row => {
      const bootstrap = await (await fetch(desktop + '/api/bootstrap')).json();
      const result = row.kind === 'bootstrap' ? bootstrap : await (await fetch(desktop + '/api/' + row.kind, {
        method: 'POST', headers: {'Content-Type': 'application/json', 'X-Hey-Boss-CSRF': bootstrap.csrf}, body: JSON.stringify(row.payload),
      })).json();
      await fetch(hub + '/api/bridge/web/' + row.id + '/result', {method: 'POST', headers, body: JSON.stringify(result)});
    }));
  } catch (error) {if (!closing) console.error(error.message);}
  finally {pumping = false;}
}, 50);
let closing = false;
function close() {
  if (closing) return; closing = true;
  clearInterval(timer); app.locals.close(); web.kill('SIGTERM');
  mobile.close(); proxy.close(); store.close();
  rmSync(binary, {force: true});
}
for (const signal of ['SIGTERM', 'SIGINT']) process.on(signal, close);
web.on('exit', close);
console.log('Issue design fixtures: desktop 48122, paired phone 52121');
