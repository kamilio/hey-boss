// Real terminal with synthetic SSH state; never runs maintenance or deletes a checkout.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, writeFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {TerminalPilot} from '../worker-tui/node_modules/terminal-pilot/dist/index.js';
import {renderTerminalPng} from '../worker-tui/node_modules/terminal-png/dist/index.js';

const root = await mkdtemp(join(tmpdir(), 'harvester-missing-ui-'));
const output = process.argv[3] ? resolve(process.argv[3]) : join(root, 'screenshots');
let pilot;
try {
  await mkdir(output, {recursive: true});
  await mkdir(join(root, 'bin'));
  const missing = {
    name: '/fixture/missing-checkout', detail: 'Missing checkout; metadata preserved',
    eligible: false, error: null,
  };
  const snapshot = {
    reporting_build: '0123456789abcdef', scan_build: '0123456789abcdef',
    observed_at: 1, last_cleanup_at: 1, phase: 'Finished',
    metrics: {disk_path: '/fixture', memory_pressure: 'Normal'}, config: {},
    processes: [], worktrees: [missing], caches: [],
    harvested_processes: 0, removed_worktrees: 0, errors: [],
  };
  const save = () => writeFile(join(root, 'snapshot.json'), JSON.stringify(snapshot));
  await save();
  await writeFile(join(root, 'bin/ssh'), `#!/bin/sh\ncat '${root}/snapshot.json'\n`, {mode: 0o700});
  pilot = await TerminalPilot.launch();
  const session = await pilot.newSession({
    command: resolve(process.argv[2]), args: ['--host', 'fixture.invalid'], cwd: root,
    cols: 100, rows: 26,
    env: {...process.env, HOME: root, HEY_BOSS_HEALTH_DIR: join(root, 'health'),
      PATH: `${root}/bin:${process.env.PATH}`, TERM: 'xterm-256color'},
  });
  const wait = text => session.waitFor(text, {scope: 'screen', timeout: 15000});
  const capture = async name => {
    await session.waitForQuiet(150);
    const screen = await session.screen();
    await renderTerminalPng(screen.rawLines.join('\n'), {output: join(output, `${name}.png`)});
    return screen.text;
  };
  await wait('Inspection errors: 0');
  await capture('overview');
  await session.type('3');
  await wait('preserved /fixture/missing-checkout');
  assert.doesNotMatch(await capture('worktrees'), /failed \/fixture\/missing-checkout/);
  await session.press('Enter');
  await wait('Missing checkout; metadata preserved');
  await capture('preservation-details-wide');
  await session.resize(48, 20);
  await wait('metadata preserved');
  await capture('preservation-details-narrow');
  await session.press('Escape');
  // Real inspection failures must still be distinct beside missing registrations.
  snapshot.worktrees.push({name: '/fixture/broken-index', detail: 'Cannot read index',
    eligible: false, error: 'Cannot read index'});
  snapshot.errors = ['/fixture/broken-index: Cannot read index'];
  snapshot.phase = 'Finished with inspection errors';
  await save();
  await session.resize(100, 26);
  await session.type('r');
  await wait('failed /fixture/broken-index');
  await wait('preserved /fixture/missing-checkout');
  await capture('preserved-and-failed');
  await session.type('1');
  await wait('Inspection errors: 1');
  await capture('real-error-overview');
  await session.type('q');
  assert.equal(await session.waitForExit({timeout: 5000}), 0);
  console.log('Passed: zero errors for missing checkout, preserved row/details at 100/48 columns, real error remains distinct and counted, clean exit.');
} finally {
  if (pilot) await pilot.close();
  await rm(root, {recursive: true, force: true});
}
