// Exercise the actual onboarding wizard in a PTY with an isolated store.
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
const root = await realpath(await mkdtemp(join(tmpdir(), 'hb-init-terminal-')));
const output = process.argv[3] && resolve(process.argv[3]);
const db = join(root, 'issues.db');
const identity = createHash('sha256').update(db).digest('hex').slice(0, 24);
const socketBase = `/tmp/hey-boss-db-${process.getuid()}/${identity}`;
const env = {...process.env, HEY_BOSS_ISSUE_DB: db, HEY_BOSS_FLEET_STATE: join(root, 'fleet'), TERM: 'xterm-256color'};
delete env.HEY_BOSS_ISSUE_HOST;
delete env.HEY_BOSS_ISSUE_PROJECT;
let pilot, service, serviceExit;
const checks = [];
function read(project, ...args) {
  const result = spawnSync(binary, ['issue', '--project', project, '--json', ...args], {cwd: root, env, encoding: 'utf8'});
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout);
}
async function capture(session, name) {
  await session.waitForQuiet(100);
  if (output) await renderTerminalPng((await session.screen()).rawLines.join('\n'), {output: join(output, `${name}.png`)});
}
try {
  if (output) await mkdir(output, {recursive: true});
  service = spawn(binary, ['fleet', 'companion'], {cwd: root, env, stdio: 'ignore'});
  serviceExit = once(service, 'exit');
  const deadline = Date.now() + 10000;
  while (!await stat(`${socketBase}.sock`).catch(() => null)) {
    assert.equal(service.exitCode, null);
    assert(Date.now() < deadline, 'Fixture service startup timed out');
    await delay(50);
  }
  pilot = await TerminalPilot.launch();
  for (const cols of [48, 80, 120]) {
    const project = `Terminal ${cols}`;
    const session = await pilot.newSession({command: binary, args: ['project', 'init', '--project', project], cwd: root, env, cols, rows: 28});
    await session.waitFor('Use pull requests?');
    assert.equal((await session.screen()).size.cols, cols);
    await capture(session, `start-${cols}`);
    await session.send('maybe\r');
    await session.waitFor('Enter y or n');
    await session.send('y\r');
    await session.waitFor('Use worktrees?');
    await session.send('n\r');
    await session.waitFor('Save project?');
    const history = (await session.history()).join('\n');
    assert(history.includes('Pull request (default)'));
    assert(history.includes('Existing checkout (default)'));
    assert(!history.includes('Dedicated worktree (default)'));
    await capture(session, `review-${cols}`);
    await session.send('p\r');
    await session.waitFor('Chief (disabled)');
    await session.waitForQuiet(100);
    await session.send('\r');
    assert.equal(await session.waitForExit({timeout: 10000}), 0);
    await capture(session, `saved-${cols}`);
    const saved = read(project, 'settings', 'show');
    assert.equal(saved.prs_enabled, true);
    assert.equal(saved.worktree_enabled, false);
    const rerun = await pilot.newSession({command: binary, args: ['project', 'init', '--project', project], cwd: root, env, cols, rows: 28});
    await rerun.waitFor('Use pull requests? [Y/n]');
    await rerun.send('\r');
    await rerun.waitFor('Use worktrees? [y/N]');
    await rerun.send('\r');
    await rerun.waitFor('Save project?');
    await rerun.send('n\r');
    assert.equal(await rerun.waitForExit({timeout: 10000}), 0);
    assert.equal(read(project, 'settings', 'show').version, saved.version);
    checks.push(`Save, prompt preview, invalid input and safe rerun at ${cols} columns`);
  }
  for (const [name, input] of [['Quit', 'q\r'], ['EOF', '\x04'], ['Interrupt', '\x03']]) {
    const session = await pilot.newSession({command: binary, args: ['project', 'init', '--project', name], cwd: root, env, cols: 80, rows: 24});
    await session.waitFor('Use pull requests?');
    await session.send(input);
    await session.waitForExit({timeout: 10000});
const rejected = spawnSync(binary, ['issue', '--project', name, 'settings', 'show'], {cwd: root, env, encoding: 'utf8'});
assert.notEqual(rejected.status, 0);
assert(rejected.stderr.includes('--project <name-or-id>'));
assert(rejected.stderr.includes('hey-boss project init'));
const rows = spawnSync('sqlite3', [db, `SELECT count(*) FROM projects WHERE name='${name}'`], {encoding: 'utf8'});
assert.equal(rows.status, 0, rows.stderr);
assert.equal(rows.stdout.trim(), '0');
    assert(!read(name, 'projects').projects.some(p => p.name === name));
    checks.push(`${name} leaves no initialized project`);
  }
const parent = join(root, 'parent');
const nested = join(parent, 'nested', 'deep');
await mkdir(nested, {recursive: true});
const rejected = await pilot.newSession({command: binary, args: ['issue', 'list'], cwd: nested, env, cols: 80, rows: 24});
await rejected.waitFor('hey-boss project init');
assert.notEqual(await rejected.waitForExit({timeout: 10000}), 0);
assert((await rejected.history()).join('\n').includes('--project <name-or-id>'));
await capture(rejected, 'uninitialized');
const initialized = spawnSync(binary, ['project', 'init', '--yes', '--prs', 'false', '--worktree', 'false', '--json'], {cwd: parent, env, encoding: 'utf8'});
assert.equal(initialized.status, 0, initialized.stderr);
const reused = await pilot.newSession({command: binary, args: ['issue', 'list', '--json'], cwd: nested, env, cols: 80, rows: 24});
assert.equal(await reused.waitForExit({timeout: 10000}), 0);
assert((await reused.history()).join('\n').includes('parent'));
const listed = spawnSync(binary, ['issue', 'list', '--json'], {cwd: nested, env, encoding: 'utf8'});
assert.equal(listed.status, 0, listed.stderr);
assert.deepEqual(JSON.parse(listed.stdout).project, JSON.parse(initialized.stdout).project);
const selected = spawnSync(binary, ['issue', '--project', 'Terminal 80', 'list', '--json'], {cwd: root, env, encoding: 'utf8'});
assert.equal(selected.status, 0, selected.stderr);
assert.equal(JSON.parse(selected.stdout).project.name, 'Terminal 80');
checks.push('Unknown directory rejects with both remedies; nested directories reuse their parent; explicit project works');
  const session = await pilot.newSession({command: binary, args: ['project', 'init', '--project', 'Terminal 80'], cwd: root, env, cols: 80, rows: 24});
  await session.waitFor('Use pull requests?'); await session.send('n\r');
  await session.waitFor('Use worktrees?'); await session.send('y\r');
  await session.waitFor('Save project?');
  const edit = spawnSync(binary, ['issue', '--project', 'Terminal 80', '--agent', 'human:qa', 'settings', 'set', '--prompt', 'Concurrent custom prompt.'], {cwd: root, env, encoding:'utf8'});
  assert.equal(edit.status, 0, edit.stderr);
  await session.send('y\r');
  assert.notEqual(await session.waitForExit({timeout: 10000}), 0);
  assert.equal(read('Terminal 80', 'settings', 'show').prompt, 'Concurrent custom prompt.');
  checks.push('Concurrent edits are preserved and reported');
  console.log(JSON.stringify({passed: checks.length, checks}));
} finally {
  if (pilot) await pilot.close();
  if (service) { service.kill('SIGTERM'); await serviceExit; }
  for (const suffix of ['.sock', '.lock', '.startup', '.log']) await rm(socketBase + suffix, {force: true});
  await rm(root, {recursive: true, force: true});
}
