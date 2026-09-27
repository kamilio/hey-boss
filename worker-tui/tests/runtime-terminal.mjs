// Real terminal review of runtime labels; synthetic queue, no worker controls.
import assert from 'node:assert/strict';
import {mkdtemp, writeFile, rm} from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {TerminalPilot} from 'terminal-pilot';
import {renderTerminalPng} from 'terminal-png';

const root = fileURLToPath(new URL('../', import.meta.url));
const temporary = await mkdtemp(path.join(os.tmpdir(), 'hey-boss-runtime-qa-'));
const pilot = await TerminalPilot.launch();
const now = Date.now();
const fixture = path.join(temporary, 'queue.mjs');
const snapshot = {
  ok: true, worker_id: 'runtime',
  workers: [{id: 'runtime', pid: 42, active: 3, config: {
    enabled: true, concurrency: 4, projects: ['named:poe-code'], directory: '/workspace/poe-code'
  }}],
  runs: [
    {id: 'days', number: 2861, seconds: 2110 * 60 + 1, title: 'Improve command performance'},
    {id: 'hours', number: 3614, seconds: 669 * 60 + 51, title: 'Unify built-in commands'},
    {id: 'minutes', number: 3579, seconds: 124, title: 'Check package exports'},
    {id: 'finished', number: 3578, seconds: 2 * 86400 + 3 * 3600, title: 'Completed runtime check'}
  ].map(run => ({...run, project_name: 'poe-code', started_at: now - run.seconds * 1000,
    finished_at: run.id === 'finished' ? now : null,
    state: run.id === 'finished' ? 'completed' : 'running',
    events: [{at: now, text: 'Verifying runtime labels at wide and narrow terminal sizes.'}]}))
};
await writeFile(fixture, '#!/usr/bin/env node\nconsole.log(' + JSON.stringify(JSON.stringify(snapshot)) + ');\n', {mode: 0o700});
const wrapper = path.join(temporary, 'terminal.sh');
await writeFile(wrapper, '#!/bin/sh\nbefore=$(stty -g)\n"$@"\ncode=$?\n[ "$(stty -g)" = "$before" ] || exit 99\nprintf "TERMINAL_RESTORED\\n"\nexit "$code"\n', {mode: 0o700});
const wait = (session, pattern) => session.waitFor(pattern, {scope: 'screen', timeout: 12000});
async function resize(session, cols, rows) {
  await session.resize(cols, rows);
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    const screen = await session.screen();
    const lines = screen.text.split('\n');
    if (rows < 12 ? screen.contains('Resize to at least') :
      lines[5]?.trimEnd().endsWith('┐') && lines[rows - 1]?.includes('q quit')) return;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw new Error('Terminal did not redraw at ' + cols + 'x' + rows);
}
async function capture(session, name) {
  await session.waitForQuiet(100);
  const screen = await session.screen();
  const output = path.join(temporary, name + '.png');
  await renderTerminalPng(screen.rawLines.join('\n'), {output});
  console.log(output);
  assert.ok(!screen.text.includes('2110m'));
  return screen;
}
try {
  const session = await pilot.newSession({command: wrapper,
    args: [path.join(root, 'target/debug/hey-boss-worker-tui'), '--binary', fixture],
    cwd: root, cols: 160, rows: 36, env: {...process.env, TERM: 'xterm-256color', COLORTERM: 'truecolor', NO_COLOR: ''}});
  await wait(session, '1d 11h');
  const wide = await capture(session, 'wide-active');
  assert.match(wide.rawLines.join('\n'), /38;2;161;175;255/, 'Capture the actual terminal palette');
  assert.equal(wide.text.split('1d 11h').length - 1, 2);
  assert.ok(wide.contains('11h 09m'));
  await session.press('ArrowDown');
  await wait(session, 'poe-code #3614 · Unify built-in commands');
  await capture(session, 'hours-selected');
  await session.press('ArrowUp');
  await wait(session, 'poe-code #2861 · Improve command performance');
  for (const [cols, rows] of [[120, 36], [80, 24], [64, 18], [48, 12]]) {
    await resize(session, cols, rows);
    await wait(session, rows === 12 ? '#2861 running' : '1d 11h');
    const screen = await capture(session, `active-${cols}x${rows}`);
    assert.ok(screen.contains(rows === 12 ? '#2861 running' : '1d 11h'));
  }
  await resize(session, 120, 36);
  await session.type('h');
  await wait(session, '2d 03h');
  assert.equal((await capture(session, 'finished')).text.split('2d 03h').length - 1, 2);
  await session.type('r');
  await wait(session, '2d 03h');
  await resize(session, 48, 12);
  await wait(session, '#3578 completed');
  await capture(session, 'finished-minimum');
  await resize(session, 30, 8);
  await wait(session, 'Resize to at least');
  await capture(session, 'too-small');
  await session.type('q');
  assert.equal(await session.waitForExit({timeout: 3000}), 0);
  assert.match((await session.history()).join('\n'), /TERMINAL_RESTORED/);
  console.log('Runtime visual checks passed.');
} finally {
  for (const session of pilot.sessions()) {
    if (session.exitCode === null) {
      await session.type('q').catch(() => {});
      await session.waitForExit({timeout: 3000}).catch(() => {
        try { process.kill(-session.pid, 'SIGKILL'); } catch (error) {
          if (error.code !== 'ESRCH') throw error;
        }
      });
    }
  }
  await pilot.close();
  if (!process.env.KEEP_RUNTIME_QA) await rm(temporary, {recursive: true, force: true});
}
