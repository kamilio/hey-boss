// Render real cleanup-check receipts in the dashboard. Only temporary fixtures are touched.
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import {mkdtemp, mkdir, writeFile, readFile, realpath, rm} from 'node:fs/promises';
import {resolve, join} from 'node:path';
import {TerminalPilot} from '../worker-tui/node_modules/terminal-pilot/dist/index.js';
import {renderTerminalPng} from '../worker-tui/node_modules/terminal-png/dist/index.js';

const binary = resolve(process.argv[2]);
const root = await realpath(await mkdtemp('/tmp/harvester-cleanup-ui-'));
const output = process.argv[3] ? resolve(process.argv[3]) : join(root, 'screenshots');
const env = {...process.env, HOME: root, HEY_BOSS_HEALTH_DIR: join(root, 'health')};
let pilot;
try {
  await mkdir(output, {recursive: true});
  await mkdir(env.HEY_BOSS_HEALTH_DIR, {recursive: true});
  await writeFile(join(env.HEY_BOSS_HEALTH_DIR, 'config.json'), JSON.stringify({workspace_roots: [root]}));
  const repository = join(root, 'repository');
  await mkdir(repository);
  const git = (...args) => {
    const result = spawnSync('git', ['-C', repository, '-c', 'core.hooksPath=/dev/null', ...args], {encoding: 'utf8'});
    assert.equal(result.status, 0, result.stderr);
  };
  git('init');
  git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--allow-empty', '-m', 'fixture');
  const clean = join(root, 'empty-ancestor', 'clean-worktree');
  await mkdir(join(root, 'empty-ancestor', '.git'), {recursive: true});
  git('worktree', 'add', '--detach', clean, 'HEAD');
  const parent = join(root, 'parent');
  git('worktree', 'add', '--detach', parent, 'HEAD');
  const locked = join(parent, 'locked-child');
  git('worktree', 'add', '--detach', locked, 'HEAD');
  git('worktree', 'lock', parent, '--reason', 'parent validation');
  const corrupt = join(root, 'corrupt-ancestor', 'corrupt-parent-child');
  await mkdir(join(root, 'corrupt-ancestor', '.git'), {recursive: true});
  await writeFile(join(root, 'corrupt-ancestor', '.git', 'HEAD'), 'corrupt');
  git('worktree', 'add', '--detach', corrupt, 'HEAD');
  const inspect = (path, expected, error = false) => {
    const result = spawnSync(binary, ['cleanup-check', path, '--json'], {env, encoding: 'utf8'});
    assert.equal(result.status, 1, result.stderr);
    const receipt = JSON.parse(result.stdout);
    assert.equal(receipt.decision, 'rejected');
    assert.match(receipt.reason, expected);
    return {name: path, detail: receipt.reason, eligible: false, error: error ? receipt.reason : null};
  };
  const build = spawnSync(binary, ['--version'], {encoding: 'utf8'}).stdout.match(/build ([a-f0-9]+)/)[1];
  const snapshot = {
    reporting_build: build, scan_build: build, observed_at: 1, phase: 'Finished',
    metrics: {disk_path: root, memory_pressure: 'Normal'}, config: {},
    processes: [], caches: [], worktrees: [
      inspect(clean, /Recently created or changed|Commits not verified/),
      inspect(locked, /Locked worktree; preserved — parent validation/),
    ], errors: [], harvested_processes: 0, removed_worktrees: 0,
  };
  const save = () => writeFile(join(root, 'snapshot.json'), JSON.stringify(snapshot));
  await save();
  await mkdir(join(root, 'bin'));
  await writeFile(join(root, 'bin', 'ssh'), `#!/bin/sh\ncat '${root}/snapshot.json'\n`, {mode: 0o700});
  pilot = await TerminalPilot.launch();
  const session = await pilot.newSession({
    command: binary, args: ['--host', 'fixture.invalid'], cwd: root, cols: 110, rows: 28,
    env: {...env, HOME: root, PATH: `${root}/bin:${process.env.PATH}`, TERM: 'xterm-256color'},
  });
  const wait = text => session.waitFor(text, {scope: 'screen', timeout: 15000});
  const capture = async name => {
    await session.waitForQuiet(200);
    const screen = await session.screen();
    await renderTerminalPng(screen.rawLines.join('\n'), {output: join(output, `${name}.png`)});
    return screen.text;
  };
  await wait('Inspection errors: 0');
  await capture('preserved-overview');
  await session.type('3');
  await wait('clean-worktree');
  assert.doesNotMatch(await capture('preserved-worktrees'), /failed/);
  await session.press('Enter');
  await wait(/Recently created|Commits not verified/);
  await capture('ordinary-checks-wide');
  await session.resize(48, 20);
  await wait('1 Home');
  await wait(/Recently created|Commits not verified/);
  await capture('ordinary-checks-narrow');
  await session.press('Escape');
  await session.press('ArrowDown');
  await session.press('Enter');
  await wait(/parent\s+validation/);
  await capture('parent-lock-narrow');
  await session.press('Escape');
  snapshot.worktrees.push(inspect(corrupt, /not a git repository/, true));
  snapshot.errors.push(snapshot.worktrees.at(-1).error);
  snapshot.phase = 'Finished with inspection errors';
  await save();
  await session.resize(110, 28);
  await wait('1 Overview');
  await session.type('r');
  await wait('corrupt-parent-child');
  assert.match(await capture('preserved-and-failed'), /failed/);
  await session.type('1');
  await wait('Inspection errors: 1');
  await capture('corrupt-metadata-overview');
  await session.type('q');
  assert.equal(await session.waitForExit({timeout: 5000}), 0);
  assert.equal(await readFile(join(root, 'corrupt-ancestor', '.git', 'HEAD'), 'utf8'), 'corrupt');
  console.log('Passed: actual safety/lock/error receipts, preserved versus failed rows, zero/one error counts, 110/48-column details, clean exit.');
} finally {
  if (pilot) await pilot.close();
  await rm(root, {recursive: true, force: true});
}
