// Exercise the real TUI without reading or changing production worker state.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, readFile, writeFile, rm} from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {TerminalPilot} from 'terminal-pilot';
import {renderTerminalPng} from 'terminal-png';

const root = fileURLToPath(new URL('../', import.meta.url));
const temporary = await mkdtemp(path.join(os.tmpdir(), 'hb-tab-order-'));
const output = path.join(root, 'target/tab-order-qa');
await mkdir(output, {recursive: true});
const statePath = path.join(temporary, 'state.json');
const fixture = path.join(temporary, 'backend.mjs');
const wrapper = path.join(temporary, 'terminal.sh');
const names = ['Alpha tools', 'Beta build', 'Gamma docs', 'Omega app', 'Zeta web'];
const workers = names.map((name, index) => ({
  id: name, pid: 42 + index, active: 1,
  config: {name, enabled: true, concurrency: 2, projects: [`named:${name}`], directory: `/work/${name}`},
  runs: [{id: `run-${name}`, project_id: `named:${name}`, number: index + 1,
    title: `${name} agent`, state: 'running', finished_at: null}], chiefs: [],
}));
const snapshot = {ok: true, project_tabs: true, workers, store: {host: 'fixture'},
  fleet: {supervisor_connection: {state: 'local'}}};
await writeFile(statePath, JSON.stringify(snapshot));
await writeFile(fixture, `#!/usr/bin/env node
import {readFileSync} from 'node:fs';
console.log(readFileSync(${JSON.stringify(statePath)}, 'utf8'));
`, {mode: 0o700});
await writeFile(wrapper, '#!/bin/sh\nbefore=$(stty -g)\n"$@"\ncode=$?\n[ "$(stty -g)" = "$before" ] || exit 99\nprintf "TERMINAL_RESTORED\\n"\nexit "$code"\n', {mode: 0o700});

const pilot = await TerminalPilot.launch();
let completed = 0;
const expected = 11;
try {
  const session = await pilot.newSession({
    command: wrapper,
    args: [path.join(root, 'target/debug/hey-boss-worker-tui'), '--projects', '--binary', fixture],
    cwd: root, cols: 110, rows: 30,
    env: {...process.env, TERM: 'xterm-256color', COLORTERM: 'truecolor', NO_COLOR: ''},
  });
  const wait = text => session.waitFor(text, {scope: 'screen', timeout: 12000});
  const capture = async name => {
    await session.waitForQuiet(100);
    const screen = await session.screen();
    assert.match(screen.rawLines.join('\n'), /38;2;161;175;255/, 'Capture actual tab highlight colors');
    await renderTerminalPng(screen.rawLines.join('\n'), {output: path.join(output, `${name}.png`)});
    return screen.text.split('\n')[0];
  };
  await wait(`${names[0]} agent`);
  const initial = await capture('wide-alpha');
  for (const name of names.slice(1)) {
    await session.press('Tab');
    await wait(`${name} agent`);
    const header = await capture(`wide-${name.toLowerCase()}`);
    assert.ok(header.includes(`[${name}]`), header);
    for (const label of names) assert.equal(header.indexOf(label), initial.indexOf(label), header);
    completed++;
  }
  const beforeRefresh = await capture('wide-before-refresh');
  snapshot.workers.reverse();
  await writeFile(statePath, JSON.stringify(snapshot));
  await session.type('r');
  await wait(`${names.at(-1)} agent`);
  assert.equal(await capture('wide-refreshed'), beforeRefresh);
  completed++;
  await session.resize(48, 16);
  await wait(`[${names.at(-1)}]`);
  const narrow = await capture('narrow-last');
  assert.ok(narrow.startsWith('‹ '), narrow);
  assert.ok(narrow.includes('Omega app'), narrow);
  assert.ok(narrow.indexOf('Omega') < narrow.indexOf('Zeta'), narrow);
  completed++;
  await session.press('Tab');
  await wait(`${names[0]} agent`);
  const wrapped = await capture('narrow-first');
  assert.ok(wrapped.includes(`[${names[0]}]`) && wrapped.trimEnd().endsWith('›'), wrapped);
  completed++;
  await session.send('\x1b[Z');
  await wait(`${names.at(-1)} agent`);
  assert.equal(await capture('narrow-backward'), narrow);
  completed++;
  await session.resize(80, 24);
  await wait(`${names.at(-1)} agent`);
  const medium = await capture('medium');
  assert.ok(names.every(name => medium.includes(name)), medium);
  assert.ok(medium.indexOf('Alpha') < medium.indexOf('Zeta'), medium);
  completed++;
  snapshot.projects = [{id: `named:${names.at(-1)}`, name: '界é👩‍💻'.repeat(30)}];
  await writeFile(statePath, JSON.stringify(snapshot));
  await session.type('r');
  await wait('…]');
  const unicode = await capture('unicode');
  assert.ok(unicode.includes('[界') && unicode.includes('…]'), unicode);
  completed++;
  await session.type('q');
  assert.equal(await session.waitForExit({timeout: 4000}), 0);
  assert.match((await session.history()).join('\n'), /TERMINAL_RESTORED/);
  assert.deepEqual(JSON.parse(await readFile(statePath, 'utf8')), snapshot);
  completed++;
  assert.equal(completed, expected);
  console.log(`COMPLETE: ${completed}/${expected} tab-order terminal scenarios; normal TUI exit. Screenshots: ${output}`);
} finally {
  await pilot.close();
  await rm(temporary, {recursive: true, force: true});
}
