// Isolated assignment and GitHub status review. No live worker or GitHub calls.
import {mkdirSync, copyFileSync, readFileSync, rmSync} from 'node:fs';
import {spawn, spawnSync} from 'node:child_process';
import {createServer, request} from 'node:http';
import {resolve} from 'node:path';

const root = resolve('output/playwright/assignment-watch');
mkdirSync(root, {recursive: true});
const binary = root + '/hey-boss-visual', database = root + '/issues.db';
for (const suffix of ['', '-wal', '-shm']) rmSync(database + suffix, {force: true});
copyFileSync(resolve('target/debug/hey-boss'), binary);
const env = {...process.env, HEY_BOSS_ISSUE_DB: database, HEY_BOSS_FLEET_STATE: root,
  HEY_BOSS_FLEET_DESIRED: root + '/absent-fleet.json',
  HEY_BOSS_INBOX_SOCKET: root + '/absent.sock'};
for (const name of ['HEY_BOSS_ISSUE_HOST', 'HEY_BOSS_ISSUE_PROJECT', 'HEY_BOSS_AGENT_ID']) delete env[name];
const project = 'named:Assignment QA';
// Start the web process first: it owns this fixture DB, avoiding a detached
// database companion when the setup CLI runs.
const web = spawn(binary, ['issue', '--project', project, 'web', '--port', '48697', '--no-discovery', '--json'], {env, stdio:['ignore','inherit','inherit']});
process.on('exit', () => web.kill('SIGTERM'));
let started = false;
for (let attempt = 0; attempt < 100; attempt++) {
  try { const response = await fetch('http://127.0.0.1:48697/'); if (response.ok) { started = true; break; } } catch {}
  if (web.exitCode !== null) break;
  await new Promise(resolve => setTimeout(resolve, 100));
}
if (!started) { web.kill('SIGTERM'); throw Error('Fixture web server did not start'); }
const cli = args => {
  const result = spawnSync(binary, ['issue', '--project', project, '--agent', 'human:boss', '--json', ...args], {env, encoding: 'utf8'});
  if (result.status) throw Error(result.stderr + result.stdout);
  return JSON.parse(result.stdout);
};
const sql = text => {
  const result = spawnSync('sqlite3', [database], {input: text, encoding: 'utf8'});
  if (result.status) throw Error(result.stderr);
};
const quote = value => "'" + String(value).replaceAll("'", "''") + "'";
for (const [index, title] of [
  'Waiting for GitHub checks',
  'Required check failed while optional review is running',
  'An agent is fixing a failed required check',
  'Queued on the development machine',
  'Attach a pull request before enabling monitoring',
  'Review feedback after all checks have finished',
  'A failed refresh retains the last known evidence',
].entries()) {
  const number = String(index + 1);
  cli(['create', '--title', title, '--body', 'Check the assignment and current GitHub status.']);
  if (![4, 5].includes(index + 1)) {
    cli(['pr', 'add', number, 'https://github.com/example/project/pull/' + number, '--purpose', 'fix']);
    cli(['assign', number, 'github', '--if-version', String(cli(['view', number]).issue.version)]);
  }
}
sql(`CREATE TABLE IF NOT EXISTS fleet_state(key TEXT PRIMARY KEY,value TEXT NOT NULL);
  INSERT OR REPLACE INTO fleet_state VALUES('machines',${quote(JSON.stringify([
    {node:'devbox', hostname:'Devbox', host:'devbox', state:'connected', workers:[]},
    {node:'laptop', hostname:'MacBook Pro', host:'local', state:'connected', workers:[]},
    {node:'offline', hostname:'Offline device with a long but readable name', host:'remote', state:'disconnected', workers:[]}
  ]))});`);
cli(['assign', '4', 'machine:devbox', '--if-version', String(cli(['view', '4']).issue.version)]);
const timestamp = Date.now();
for (const number of [1, 2, 3, 6, 7]) {
  const failed = [2, 3].includes(number);
  const evidence = {repository:'example/project', number, head:'0123456789abcdef', complete:number === 6 || number === 7,
    required:[{context:'Unit tests',state:failed?'failure':'satisfied',url:'https://github.com/example/project/actions/runs/123'},
      {context:'Build and type checks',state:'satisfied',url:'https://github.com/example/project/actions/runs/124'}],
    reviews:number === 6 ? [{body:'Please handle a connection drop while saving. The draft should remain available after reconnecting.',html_url:'https://github.com/example/project/pull/6#review'}] : [],
    source_errors:[],ci_errors:[],policy_errors:[]};
  const url = 'https://github.com/example/project/pull/' + number;
  const status = {prs:{[url]:{head:evidence.head,checked_at:timestamp,evidence,...(number===7?{error:'GitHub rate limit reached. Monitoring will retry after the cooldown.'}:{})}}};
  if (failed || number === 6) status.event = 'fixture-event-' + number;
  sql(`INSERT INTO issue_github_watches VALUES(${quote(project)},${number},${quote(JSON.stringify(status))});`);
}
sql(`INSERT INTO agents(id,metadata,last_seen) VALUES('codex:fixture',${quote(JSON.stringify({id:'codex:fixture',kind:'codex',machine:'devbox',host:'Devbox',cwd:root,session_id:'fixture',source:'visual fixture'}))},${timestamp});
  UPDATE issues SET assignee=NULL WHERE number IN (2,6);
  UPDATE issues SET assignee='codex:fixture' WHERE number=3;`);

const assets = new Set(['app.js','app.css','assignments.js','assignments.css','components.js']);
const proxy = createServer((req, res) => {
  const file = req.url.split('?')[0].slice(1);
  if (file === 'api/inbox') {
    res.setHeader('Content-Type','application/json');
    res.end(JSON.stringify({ok:true,tasks:[],unread:0}));
    return;
  }
  if (assets.has(file)) {
    res.setHeader('Content-Type', file.endsWith('.js') ? 'application/javascript' : 'text/css');
    res.end(readFileSync(resolve('src/issues/web', file)));
    return;
  }
  const headers = {...req.headers, host:'127.0.0.1:48697'};
  delete headers['sec-fetch-site'];
  for (const key of ['origin','referer']) if (headers[key]) headers[key] = headers[key].replace('127.0.0.1:48698','127.0.0.1:48697');
  const upstream = request({hostname:'127.0.0.1',port:48697,path:req.url,method:req.method,headers}, reply => {res.writeHead(reply.statusCode,reply.headers);reply.pipe(res);});
  upstream.on('error', () => {res.writeHead(502);res.end('Fixture starting');});
  req.pipe(upstream);
});
proxy.listen(48698, '127.0.0.1');
let closing = false;
function close() {
  if (closing) return;
  closing = true;
  web.kill('SIGTERM');
  proxy.close();
}
for (const signal of ['SIGTERM','SIGINT']) process.on(signal, close);
web.on('exit', () => { rmSync(binary, {force:true}); close(); });
console.log('Assignment watcher fixture: http://127.0.0.1:48698');
