// Real terminal with synthetic SSH responses; never runs maintenance.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, writeFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
import {TerminalPilot} from '../worker-tui/node_modules/terminal-pilot/dist/index.js';
import {renderTerminalPng} from '../worker-tui/node_modules/terminal-png/dist/index.js';

const root = await mkdtemp(join(tmpdir(), 'harvester-build-ui-'));
const output = resolve(process.argv[3] || 'output/playwright/issue366');
let pilot;
try {
  await mkdir(output, {recursive: true});
  await mkdir(join(root, 'bin'));
  const snapshot = {
    reporting_build: 'fedcba9876543210', scan_build: '0123456789abcdef',
    observed_at: 1, last_cleanup_at: 1, phase: 'Inspecting worktrees',
    metrics: {disk_path: '/fixture', memory_pressure: 'Normal'}, config: {},
    processes: [], worktrees: [], harvested_processes: 0, removed_worktrees: 0,
    caches: [{name: '24-hour cache expiration', detail: 'Git checkouts, SQLite databases and application bundles preserved', eligible: false}],
    errors: [], cache_progress: {visited_this_cycle: 90000, slice_millis: 25000, roots_pending: 12},
  };
  const save = () => writeFile(join(root, 'snapshot.json'), JSON.stringify(snapshot));
  await save();
  await writeFile(join(root, 'bin/ssh'), `#!/bin/sh\ncat '${root}/snapshot.json'\n`, {mode: 0o700});
  pilot = await TerminalPilot.launch();
  const session = await pilot.newSession({
    command: resolve(process.argv[2]), args: ['--host', 'fixture.invalid'], cwd: root,
    cols: 100, rows: 26,
    env: {...process.env, HOME: root, HEY_BOSS_HEALTH_DIR: join(root, 'health'), PATH: `${root}/bin:${process.env.PATH}`, TERM: 'xterm-256color'},
  });
  const wait = text => session.waitFor(text, {scope: 'screen', timeout: 15000});
  const capture = async name => {
    await session.waitForQuiet(150);
    const screen = await session.screen();
    await renderTerminalPng(screen.rawLines.join('\n'), {output: join(output, `${name}.png`)});
    return screen.text;
  };
  await wait('Binary build: fedcba9876543210');
  await wait('Last scan build: 0123456789abcdef');
  await wait('90000 entries');
  await capture('different-builds');
  await session.resize(48, 20);
  await delay(500);
  await wait('Last scan build: 0123456789abcdef');
  await capture('different-builds-narrow');
  delete snapshot.scan_build;
  await save(); await session.type('r');
  await wait('Last scan build: unknown');
  await session.press('ArrowDown'); await session.press('Enter');
  await wait('awaiting a new scan');
  await capture('legacy-scan-details-narrow');
  await session.press('Escape');
  snapshot.scan_build = snapshot.reporting_build;
  await save(); await session.type('r');
  await wait('Last scan build: fedcba9876543210');
  await capture('verified-build-narrow');
  await session.type('4'); await wait('24-hour cache expiration');
  await session.press('Enter'); await wait('bundles preserved');
  await capture('cache-preservation-details');
  await session.press('Escape'); await session.type('q');
  assert.equal(await session.waitForExit({timeout: 5000}), 0);
  console.log('Passed: build mismatch, legacy status, matching builds, 100/48-column layouts, cache details, clean exit.');
} finally {
  if (pilot) await pilot.close();
  await rm(root, {recursive: true, force: true});
}
