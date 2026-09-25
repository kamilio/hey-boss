// Real CLI output in a PTY, with an isolated issue store and no browser sessions.
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
const output = resolve('out/issue250-terminal');
const root = await mkdtemp(join(tmpdir(), 'hey-boss-settings-visual-'));
const db = join(await realpath(root), 'issues.db');
const identity = createHash('sha256').update(db).digest('hex').slice(0, 24);
const socketBase = `/tmp/hey-boss-db-${process.getuid()}/${identity}`;
const env = {...process.env, HEY_BOSS_ISSUE_DB: db, HEY_BOSS_FLEET_STATE: join(root, 'fleet'), TERM: 'xterm-256color'};
delete env.HEY_BOSS_ISSUE_HOST;
const prefix = ['issue', '--project', 'Settings QA', '--agent', 'human:qa'];
const checks = [];
let pilot;
let service;
let serviceExit;
function run(args) {
  const result = spawnSync(binary, [...prefix, ...args], {cwd: root, env, encoding: 'utf8'});
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  return result.stdout;
}
try {
  await mkdir(output, {recursive: true});
  service = spawn(binary, ['fleet', 'companion'], {cwd: root, env, stdio: 'ignore'});
  serviceExit = once(service, 'exit');
  const deadline = Date.now() + 10000;
  while (!await stat(`${socketBase}.sock`).catch(() => null)) {
    assert.equal(service.exitCode, null, 'Fixture service exited before startup');
    assert(Date.now() < deadline, 'Fixture database startup timed out');
    await delay(50);
  }
  run(['create', '--title', 'Readable issue', '--body', 'Keep the issue details easy to scan.']);
  pilot = await TerminalPilot.launch();
  for (const cols of [48, 80, 120]) {
    for (const mode of ['disabled', 'prs', 'settings']) {
      run(['settings', 'set', mode === 'disabled' ? '--no-prs' : '--prs-enabled',
        '--worktree', '--chief', '--chief-prompt', 'Organize.\nKeep Markdown.', '--prompt', 'Shared instructions.']);
      const args = mode === 'settings' ? ['settings', 'show'] : ['view', '1'];
      const session = await pilot.newSession({command: binary, args: [...prefix, ...args], cwd: root, env, cols, rows: 24});
      assert.equal(await session.waitForExit({timeout: 10000}), 0);
      await session.waitForQuiet(100);
      const screen = await session.screen();
      assert.equal(screen.size.cols, cols);
      assert(screen.text.includes('Settings QA (named:Settings QA)'));
      assert(!screen.text.includes('null') && !screen.text.includes(': false'));
      assert(!screen.text.includes('Worktrees enabled:'));
      if (mode === 'settings') {
        assert(screen.text.includes('Worktrees allowed: true'));
        assert(screen.text.includes('Chief enabled: true'));
        assert(screen.text.includes('Shared prompt: Shared instructions.'));
        assert(screen.text.includes('Subtask scheduling: sequential'));
      } else {
        assert(screen.text.includes('#1 [open] Readable issue'));
        assert(screen.text.includes('Comments: 0 shown · 0 total'));
        assert(!screen.text.includes('Worktrees') && !screen.text.includes('Chief'));
      }
      assert.equal(screen.text.includes('PRs enabled: true'), mode !== 'disabled');
      await renderTerminalPng(screen.rawLines.join('\n'), {output: join(output, `${mode}-${cols}.png`)});
      checks.push(`${mode} at ${cols} columns`);
    }
  }
  assert.equal(checks.length, 9);
  console.log(JSON.stringify({normalCompletion: true, completed: 9, expected: 9, checks}));
} finally {
  if (pilot) await pilot.close();
  if (service) {
    service.kill('SIGTERM');
    const [code, signal] = await serviceExit;
    assert.equal(code, 0, `Fixture service did not stop normally: ${signal}`);
  }
  for (const suffix of ['.sock', '.lock', '.startup', '.log']) await rm(socketBase + suffix, {force: true});
  await rm(root, {recursive: true, force: true});
}
