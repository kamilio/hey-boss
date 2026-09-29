// Isolated CLI/terminal qualification. Leave the fixture UI open until Ctrl+C.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, rm, realpath, stat} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {spawn, spawnSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {once} from 'node:events';
import {setTimeout as delay} from 'node:timers/promises';
import {TerminalPilot} from '../worker-tui/node_modules/terminal-pilot/dist/index.js';
import {renderTerminalPng} from '../worker-tui/node_modules/terminal-png/dist/index.js';

const binary = resolve(process.argv[2] || 'target/debug/hey-boss');
const output = resolve('output/playwright/issue705');
const root = await realpath(await mkdtemp(join(tmpdir(), 'hey-boss-recovery-')));
const database = join(root, 'issues.db');
const socketBase = `/tmp/hey-boss-db-${process.getuid()}/${createHash('sha256').update(database).digest('hex').slice(0, 24)}`;
const env = {...process.env, HEY_BOSS_ISSUE_DB: database, HEY_BOSS_FLEET_STATE: join(root, 'fleet'),
  HEY_BOSS_INBOX_SOCKET: join(root, 'inbox.sock'), TERM: 'xterm-256color'};
for (const key of ['HEY_BOSS_ISSUE_HOST', 'HEY_BOSS_ISSUE_PROJECT', 'CODEX_THREAD_ID']) delete env[key];
const prefix = ['issue', '--project', 'Transaction recovery QA', '--agent', 'human:qa'];
const children = [];
let pilot;
const start = args => {
  const child = spawn(binary, args, {cwd: root, env, stdio: 'ignore'});
  const exited = once(child, 'exit');
  children.push({child, exited});
  return child;
};
function run(args, success = true) {
  const result = spawnSync(binary, [...prefix, ...args, '--json'], {cwd: root, env, encoding: 'utf8', timeout: 15000});
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status === 0, success, result.stdout + result.stderr);
  return JSON.parse(result.stdout);
}
try {
  await mkdir(output, {recursive: true});
  const owner = start(['fleet', 'companion']);
  for (let attempt = 0; !await stat(socketBase + '.sock').catch(() => null); attempt++) {
    assert(attempt < 300 && owner.exitCode === null, 'Fixture owner failed to start');
    await delay(50);
  }
  run(['create', '--title', 'Prerequisite', '--draft']);
  const creation = ['create', '--title', 'Dependent issue saved exactly once', '--body',
    'The original specification survives an uncertain response.\n\n- One issue\n- One prerequisite',
    '--blocked-by', '1', '--request-id', 'dependent-once'];
  const created = run(creation);
  assert.equal(created.issue.number, 2);
  assert.equal(created.issue.state, 'blocked');
  assert.deepEqual(run(creation), created);
  assert.equal(run(['request', 'dependent-once']).request.response.issue.number, 2);
  assert.equal(run(['request', 'missing']).request.state, 'not_recorded');
  run(['create', '--title', 'Rejected dependency', '--blocked-by', '999', '--request-id', 'rejected'], false);
  assert.equal(run(['request', 'rejected']).request.state, 'not_recorded');
  const later = run(['create', '--title', 'Created after the failed dependency', '--draft']);
  assert.equal(later.issue.number, 3);
  assert.equal(run(['view', '2']).issue.title, created.issue.title);
  assert.equal(run(['list', '--state', 'all', '--all']).issues.length, 3);
  pilot = await TerminalPilot.launch();
  let checks = 0;
  for (const cols of [48, 80, 120]) {
    for (const id of ['dependent-once', 'missing']) {
      const session = await pilot.newSession({command: binary, args: [...prefix, 'request', id], cwd: root, env, cols, rows: 30});
      assert.equal(await session.waitForExit({timeout: 15000}), 0);
      await session.waitForQuiet(100);
      const screen = await session.screen();
      assert.equal(screen.size.cols, cols);
      assert(screen.text.includes(id));
      assert(screen.text.includes(id === 'missing' ? 'No saved receipt' : 'Recorded'));
      assert(!screen.text.includes('undefined') && !screen.text.includes('null'));
      assert(screen.text.replace(/\s/g, '').includes(id === 'missing' ? 'doesnotprove' : 'Savedresult:#2'));
      await renderTerminalPng(screen.rawLines.join('\n'), {output: join(output, `${id}-${cols}.png`)});
      checks++;
    }
  }
  await pilot.close();
  pilot = null;
  start([...prefix, 'web', '--port', '59705', '--no-discovery', '--json']);
  let boot;
  for (let attempt = 0; !boot; attempt++) {
    assert(attempt < 300, 'Fixture web server failed to start');
    try { boot = await (await fetch('http://127.0.0.1:59705/api/bootstrap')).json(); }
    catch { await delay(50); }
  }
  console.log(JSON.stringify({terminalChecks: checks, cliChecks: 'passed', url: 'http://127.0.0.1:59705/', project: boot.project.id}));
  await Promise.race([once(process, 'SIGINT'), once(process, 'SIGTERM')]);
} finally {
  if (pilot) await pilot.close();
  for (const {child, exited} of children.reverse()) {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
    await exited;
  }
  for (const suffix of ['.sock', '.lock', '.startup', '.log']) await rm(socketBase + suffix, {force: true});
  await rm(root, {recursive: true, force: true});
  console.log('Fixture services stopped; temporary database and socket files removed.');
}
