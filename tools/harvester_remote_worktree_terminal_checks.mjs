// Real PTY with synthetic SSH status; never dispatches cleanup.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, writeFile, readFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
import {TerminalPilot} from '../worker-tui/node_modules/terminal-pilot/dist/index.js';
import {renderTerminalPng} from '../worker-tui/node_modules/terminal-png/dist/index.js';

const binary = resolve(process.argv[2] || 'target/debug/hey-harvester');
const output = resolve(process.argv[3]);
const root = await mkdtemp(join(tmpdir(), 'harvester-remote-ui-'));
let pilot;
try {
  await mkdir(output, {recursive: true});
  await mkdir(join(root, 'bin'));
  const reasons = [
    'Clean and unused for 4 hours; ownership and recovery checks passed',
    'Commits not verified on a remote branch; preserved',
    'Present skip-worktree file may hide edits; index flags preserved',
    'Locked worktree; preserved — issue 697; active owner',
    'Cannot verify HEAD on a live remote branch; preserved',
    'Open in a process or agent; preserved',
    'Source or Git activity within 4 hours; preserved',
  ];
  const snapshot = {
    reporting_build: 'fixture697', scan_build: 'fixture697', observed_at: 1,
    last_cleanup_at: 1, metrics: {disk_path: '/fixture', memory_pressure: 'Normal'},
    config: {}, processes: [], harvested_processes: 0, removed_worktrees: 0, errors: [],
    worktrees: reasons.map((detail, i) => ({
      name: `/Users/example/Workspace/project-${['published-sparse', 'unpushed', 'hidden-edits', 'owned', 'offline', 'active', 'recent'][i]}`,
      detail, eligible: i === 0,
      worktree: {path: `/fixture/worktree-${i}`, age_seconds: 86400, repository: 'example/project', github_url: null},
    })),
  };
  await writeFile(join(root, 'snapshot.json'), JSON.stringify(snapshot));
  await writeFile(join(root, 'bin/ssh'), `#!/bin/sh\nprintf '%s\\n' "$*" >> '${root}/calls'\ncat '${root}/snapshot.json'\n`, {mode: 0o700});
  pilot = await TerminalPilot.launch();
  const session = await pilot.newSession({command: binary, args: ['--host', 'fixture.invalid'], cwd: root, cols: 100, rows: 26,
    env: {...process.env, HOME: root, HEY_BOSS_HEALTH_DIR: join(root, 'health'), PATH: `${root}/bin:${process.env.PATH}`, TERM: 'xterm-256color'}});
  const wait = async text => {
    try {
      return await session.waitFor(text, {scope: 'screen', timeout: 30000});
    } catch (error) {
      console.error((await session.screen()).text);
      throw error;
    }
  };
  const capture = async name => {
    await session.waitForQuiet(150);
    const screen = await session.screen();
    await renderTerminalPng(screen.rawLines.join('\n'), {output: join(output, `${name}.png`)});
    return screen.text;
  };
  await wait('Automatic: false');
  await session.type('3'); await wait('published-sparse');
  await capture('worktree-list');
  for (let i = 0; i < reasons.length; i++) {
    await session.press('Enter'); await wait('Selected entry');
    for (const [cols, rows] of [[100, 26], [48, 20]]) {
      await session.resize(cols, rows); await delay(300);
      const screen = await capture(`reason-${i}-${cols}`);
      const normalized = screen.replace(/[│\n]/g, ' ').replace(/\s+/g, ' ');
      assert(normalized.includes(reasons[i]), `Missing preservation detail: ${reasons[i]}\n${screen}`);
      assert(screen.includes('Esc Back'));
    }
    await session.press('Escape'); await wait('Enter Details');
    await session.resize(100, 26); await delay(300);
    if (i < reasons.length - 1) await session.press('ArrowDown');
  }
  await session.type('x'); await wait('Safety checks still apply');
  await capture('cancel-removal');
  await session.type('n'); await wait('Enter Details');
  await session.type('q');
  assert.equal(await session.waitForExit({timeout: 5000}), 0);
  const calls = await readFile(join(root, 'calls'), 'utf8');
  assert(!calls.includes('remove-worktree') && !calls.includes("'clean'"));
  console.log('Passed: published/sparse, unpushed, hidden edits, ownership, offline, active and recent details at 100/48 columns; cancellation; no mutations; clean exit.');
} finally {
  if (pilot) await pilot.close();
  await rm(root, {recursive: true, force: true});
}
