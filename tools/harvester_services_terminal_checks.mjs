// Real terminal dashboard, synthetic SSH inventory; never runs maintenance.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, writeFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
import {TerminalPilot} from '../worker-tui/node_modules/terminal-pilot/dist/index.js';
import {renderTerminalPng} from '../worker-tui/node_modules/terminal-png/dist/index.js';

const root = await mkdtemp(join(tmpdir(), 'harvester-services-ui-'));
const output = resolve(process.argv[3]);
let pilot;
try {
  await mkdir(output, {recursive: true});
  await mkdir(join(root, 'bin'));
  const failure = 'Service ownership inspection unavailable or incomplete; processes preserved';
  const snapshot = {
    reporting_build: '0123456789abcdef', scan_build: '0123456789abcdef',
    observed_at: 1, last_cleanup_at: 1, phase: 'Waiting for the next check',
    metrics: {disk_path: '/fixture', memory_pressure: 'Critical'},
    config: {automatic: true, aggressive: true, harvest_processes: true},
    processes: [
      {name: 'Protected service · PID 101', detail: 'Managed service or descendant; preserved', eligible: false},
      {name: 'Orphan developer runtime · PID 102', detail: 'TERM was sent; no KILL sent: Managed service or descendant; preserved', eligible: false},
      {name: 'Protected service · PID 103', detail: failure, error: failure, eligible: false},
    ],
    worktrees: [], caches: [], harvested_processes: 0, removed_worktrees: 0,
    errors: [failure], activity: [{at: 1, category: 'process', message: 'Protected service · PID 101 — Managed service or descendant; preserved'}],
  };
  const save = () => writeFile(join(root, 'snapshot.json'), JSON.stringify(snapshot));
  await save();
  await writeFile(join(root, 'bin/ssh'), `#!/bin/sh\ncat '${root}/snapshot.json'\n`, {mode: 0o700});
  pilot = await TerminalPilot.launch();
  const session = await pilot.newSession({
    command: resolve(process.argv[2]), args: ['--host', 'fixture.invalid'], cwd: root,
    cols: 110, rows: 28,
    env: {...process.env, HOME: root, HEY_BOSS_HEALTH_DIR: join(root, 'health'), PATH: `${root}/bin:${process.env.PATH}`, TERM: 'xterm-256color'},
  });
  const wait = text => session.waitFor(text, {scope: 'screen', timeout: 15000});
  const capture = async name => {
    await session.waitForQuiet(150);
    const screen = await session.screen();
    await renderTerminalPng(screen.rawLines.join('\n'), {output: join(output, `${name}.png`)});
    return screen.text;
  };
  await wait('Inspection errors: 1');
  await capture('overview');
  await session.type('2');
  await wait('preserved Protected service');
  await wait('failed Protected service');
  await capture('services-wide');
  for (const width of [110, 48]) {
    await session.resize(width, 28); await delay(300);
    // Page switching resets selection; inspect all three distinct outcomes.
    await session.type('1'); await delay(150); await session.type('2'); await delay(150);
    for (let row = 0; row < 3; row++) {
      await session.press('Enter'); await wait('Selected entry');
      const text = await capture(`service-${row}-${width}`);
      assert.match(text, row === 0 ? /Managed service/ : row === 1 ? /no KILL sent/ : /inspection unavailable/);
      assert.match(text, /preserved/);
      await session.press('Escape'); await delay(200);
      await session.press('ArrowDown'); await delay(200);
    }
  }
  await session.type('5'); await wait('Protected service');
  await session.press('Enter'); await wait('Managed service');
  await capture('activity-narrow');
  await session.press('Escape'); await delay(200);
  snapshot.errors = []; snapshot.processes.pop();
  await save(); await session.type('1'); await session.type('r');
  await wait('Inspection errors: 0');
  await capture('recovered-narrow');
  await session.type('q');
  assert.equal(await session.waitForExit({timeout: 5000}), 0);
  console.log('Passed: service preservation, blocked escalation, inspection failure, activity, recovery, 110/48-column details, normal exit.');
} finally {
  if (pilot) await pilot.close();
  await rm(root, {recursive: true, force: true});
}
