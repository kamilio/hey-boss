// Real CLI, private queue and fixture agent: no production workers are controlled.
import assert from 'node:assert/strict';
import {spawn, execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdtemp, mkdir, readFile, writeFile, rm, realpath, access} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {TerminalPilot} from 'terminal-pilot';
import {renderTerminalPng} from 'terminal-png';

const repo = fileURLToPath(new URL('../../', import.meta.url));
const binary = process.env.HEY_BOSS_TEST_BINARY || path.join(repo, 'target/debug/hey-boss');
const temporary = await realpath(await mkdtemp(path.join(os.tmpdir(), 'hb-resume-')));
const checkout = path.join(temporary, 'checkout');
const output = path.join(repo, 'worker-tui/target/resume-qa');
await mkdir(checkout);
await mkdir(output, {recursive: true});
const database = path.join(temporary, 'issues.db');
const socketId = createHash('sha256').update(database).digest('hex').slice(0, 24);
const socketBase = path.join('/tmp', `hey-boss-db-${process.getuid()}`, socketId);
const env = {...process.env, TERM: 'xterm-256color',
  HEY_BOSS_ISSUE_DB: database, HEY_BOSS_FLEET_STATE: path.join(temporary, 'state'),
  HEY_BOSS_FLEET_DESIRED: path.join(temporary, 'desired.json'),
  HEY_BOSS_FLEET_BINARY: binary, HEY_BOSS_TEST_CLI: binary,
  HEY_BOSS_CODEX: path.join(repo, 'tests/fixtures/codex-drain.mjs'),
  HEY_BOSS_INBOX_SOCKET: path.join(temporary, 'absent.sock')};
delete env.HEY_BOSS_ISSUE_HOST;
delete env.HEY_BOSS_ISSUE_PROJECT;
delete env.HEY_BOSS_FLEET_MANAGED;
await writeFile(env.HEY_BOSS_FLEET_DESIRED, JSON.stringify({machines: {local: {workers: []}}}));
const execute = promisify(execFile);
const cli = async args => JSON.parse((await execute(binary, args, {env, cwd: checkout, timeout: 60000})).stdout);
const status = () => cli(['auto-workers', '--json', 'status']);
const until = async (predicate, label) => {
  const deadline = Date.now() + 20000;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw Error(`Timed out: ${label}`);
};
const service = spawn(binary, ['fleet', 'companion'], {env, cwd: checkout, stdio: 'ignore'});
const serviceExit = new Promise(resolve => service.once('exit', (code, signal) => resolve({code, signal})));
let pilot;
let added = false;
try {
  await until(async () => {try {await access(socketBase + '.sock'); return true;} catch {return false;}}, 'private database service');
  await cli(['issue', '--json', '--agent', 'human:resume-test', 'create', '--title', 'Keep building while pickup resumes']);
  await cli(['auto-workers', '--json', 'add', '--id', 'resume-fixture', '--name', 'Builder', '-C', checkout]);
  added = true;
  await until(async () => {try {await access(path.join(checkout, 'agent-started')); return true;} catch {return false;}}, 'fixture agent start');
  const before = (await status()).workers[0];
  const agent = Number(await readFile(path.join(checkout, 'agent-started'), 'utf8'));
  await cli(['worker', '--json', 'pause', 'resume-fixture']);
  assert.equal((await status()).workers[0].config.enabled, false);
  pilot = await TerminalPilot.launch();
  const open = args => pilot.newSession({command: binary, args, cwd: checkout, env, cols: 120, rows: 30});
  const capture = async (session, name) => {
    await session.waitForQuiet(100);
    const screen = await session.screen();
    await writeFile(path.join(output, name + '.txt'), screen.text);
    await renderTerminalPng(screen.rawLines.join('\n'), {output: path.join(output, name + '.png')});
    return screen;
  };
  const observer = await open(['auto-workers', 'status']);
  await observer.waitFor('0 available / 0 slots', {scope: 'screen', timeout: 15000});
  await observer.type('w');
  await observer.waitFor('Finishing work before pause', {scope: 'screen', timeout: 15000});
  await capture(observer, 'paused-observer');
  await observer.press('Escape');
  await observer.type('q');
  assert.equal(await observer.waitForExit({timeout: 5000}), 0);
  assert.equal((await status()).workers[0].config.enabled, false, 'Observation resumed pickup');
  const resumed = await open(['auto-workers']);
  await resumed.waitFor('0 available / 1 slots', {scope: 'screen', timeout: 15000});
  await resumed.type('w');
  await resumed.waitFor('BUSY', {scope: 'screen', timeout: 15000});
  await capture(resumed, 'resumed-worker');
  await resumed.press('Escape');
  for (const [cols, rows] of [[120, 30], [80, 24], [48, 16]]) {
    await resumed.resize(cols, rows);
    await resumed.waitFor('0 available / 1 slots', {scope: 'screen', timeout: 5000});
    const screen = await capture(resumed, `resumed-${cols}`);
    assert.ok(!screen.contains('Finishing work before pause'));
    assert.ok(screen.contains('1 active'));
  }
  await resumed.type('q');
  assert.equal(await resumed.waitForExit({timeout: 5000}), 0);
  const after = (await status()).workers[0];
  assert.equal(after.pid, before.pid);
  assert.equal(after.active, 1);
  assert.equal(after.config.concurrency, before.config.concurrency);
  assert.equal(after.config.enabled, true);
  assert.equal(after.intent, 'running');
  process.kill(agent, 0);
  await writeFile(path.join(checkout, 'release-agent'), '');
  await until(async () => (await status()).workers[0].active === 0, 'agent normal completion');
  const issue = await cli(['issue', '--json', 'view', '1']);
  assert.equal(issue.issue.state, 'closed');
  const completed = await cli(['worker', '--json', '--id', 'resume-fixture', '--history', '20', 'status']);
  assert.equal(completed.runs.length, 1);
  assert.equal(completed.runs[0].state, 'completed');
  assert.ok(completed.runs[0].finished_at);
  console.log('COMPLETE: 6/6 — observation, resume, 3 terminal sizes, preserved agent completed normally.');
} finally {
  try {
    if (pilot) await pilot.close();
    if (added) {
      await cli(['worker', '--json', 'stop', 'resume-fixture']);
      await until(async () => !(await cli(['worker', '--json', 'status'])).workers.some(w => w.pid || w.active), 'fixture worker exit');
    }
  } finally {
    service.kill('SIGTERM');
    await serviceExit;
    await rm(temporary, {recursive: true, force: true});
    for (const suffix of ['.sock', '.lock', '.log', '.startup']) await rm(socketBase + suffix, {force: true});
  }
}
