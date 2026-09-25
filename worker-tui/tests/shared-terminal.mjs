// Synthetic PTY walkthrough: shared workers never touch the live fleet.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, readFile, writeFile, rm} from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {TerminalPilot} from 'terminal-pilot';
import {renderTerminalPng} from 'terminal-png';

const root = fileURLToPath(new URL('../', import.meta.url));
const temporary = await mkdtemp(path.join(os.tmpdir(), 'hb-shared-tabs-'));
const output = path.join(root, 'target/project-tabs-qa');
await mkdir(output, {recursive:true});
const state = path.join(temporary, 'state.json');
const fixture = path.join(temporary, 'backend.mjs');
const run = (id, project, finished = null) => ({id, project_id:`named:${project}`, project_name:project,
  number:7, title:`${project} task`, state:finished ? 'completed' : 'running', finished_at:finished,
  events:[{text:`Checking ${project} without changing the live fleet.`}]});
const worker = (id, projects, runs) => ({id, pid:42, active:runs.filter(r => !r.finished_at).length,
  config:{name:id, enabled:true, concurrency:3, projects:projects.map(p => `named:${p}`),
    directories:Object.fromEntries(projects.map(p => [`named:${p}`, `/work/${p}`]))}, runs, chiefs:[]});
const snapshot = {ok:true, project_tabs:true, fleet:{supervisor_connection:{state:'local'}}, workers:[
  worker('Dedicated Alpha', ['Alpha'], [run('alpha', 'Alpha')]),
  worker('Dedicated Beta', ['Beta'], [run('beta', 'Beta')]),
  worker('Shared one', ['Tools', 'Docs', 'Proxy', 'Search', 'Notes'], [run('tools', 'Tools'), run('docs', 'Docs', 42)]),
  worker('Shared two', ['Notes', 'Proxy', 'Search', 'Docs', 'Tools'], [run('proxy', 'Proxy')]),
]};
await writeFile(state, JSON.stringify(snapshot));
await writeFile(fixture, `#!/usr/bin/env node
import {readFileSync} from 'node:fs';
if(process.argv[4] && process.argv[4] !== 'status') throw Error('Unexpected mutation');
console.log(readFileSync(${JSON.stringify(state)}, 'utf8'));
`, {mode:0o700});
const pilot = await TerminalPilot.launch();
const wait = (session, text) => session.waitFor(text, {scope:'screen', timeout:12000});
async function open(session, key, title) {
  const deadline = Date.now() + 12000;
  while (Date.now() < deadline) {
    await session.type(key);
    try { await session.waitFor(title, {scope:'screen', timeout:300}); return; } catch {}
  }
  throw Error(`Could not open ${title}`);
}
const capture = async (session, name) => {
  await session.waitForQuiet(100);
  const screen = await session.screen();
  assert.match(screen.rawLines.join('\n'), /38;2;161;175;255/, 'Visual QA must retain the actual accent colors');
  await writeFile(path.join(output, `shared-${name}.txt`), screen.text);
  await renderTerminalPng(screen.rawLines.join('\n'), {output:path.join(output, `shared-${name}.png`)});
};
try {
  const session = await pilot.newSession({command:path.join(root, 'target/debug/hey-boss-worker-tui'),
    args:['--projects', '--binary', fixture], cwd:root, cols:120, rows:36,
    env:{...process.env, TERM:'xterm-256color', COLORTERM:'truecolor', NO_COLOR:''}});
  await wait(session, '[Alpha]');
  await session.press('Tab'); await wait(session, '[Beta]');
  await session.press('Tab'); await wait(session, '[Shared · 2 workers · 5 projects]');
  await wait(session, 'Tools task'); await wait(session, 'Proxy task');
  assert.ok(!(await session.screen()).contains('Alpha task'));
  await capture(session, 'wide');
  await session.press('ArrowDown'); await wait(session, 'Shared two');
  await open(session, 'p', 'Pause worker?'); await wait(session, 'Shared two');
  await capture(session, 'confirm'); await session.press('Escape');
  // Wait for Escape to dismiss the modal before sending another key; otherwise
  // the PTY can combine Escape+h into an Alt+h event on a busy runner.
  await wait(session, /^(?![\s\S]*Pause worker\?)[\s\S]*Tools task/);
  await session.type('h'); await wait(session, 'Docs task');
  assert.ok(!(await session.screen()).contains('Proxy task'));
  await capture(session, 'history');
  await session.type('h'); await wait(session, 'Tools task');
  for (const [cols, rows] of [[100,28], [80,24], [64,18], [48,12]]) {
    await session.resize(cols, rows); await wait(session, '[Shared · 2 workers · 5 projects]');
    // The header can survive resize before Ratatui redraws the body.
    await wait(session, cols >= 64 ? 'Checking Tools without changing the live fleet.' : 'Tools task');
    await capture(session, `${cols}x${rows}`);
  }
  await session.resize(120,36);
  // Wait for the expanded body before sending a control key. The tab header
  // also exists in the previous 48x12 frame and cannot prove resize completed.
  await wait(session, 'Activity · PgUp/PgDn');
  await session.type('w'); await wait(session, 'Shared workers');
  assert.ok(!(await session.screen()).contains('Dedicated Alpha'));
  await capture(session, 'workers'); await session.press('Escape');
  snapshot.workers.reverse();
  await writeFile(state, JSON.stringify(snapshot));
  await session.type('r'); await wait(session, '[Shared · 2 workers · 5 projects]');
  await session.press('Tab'); await wait(session, '[Alpha]');
  await session.send('\x1b[Z'); await wait(session, '[Shared · 2 workers · 5 projects]');
  snapshot.workers = snapshot.workers.filter(w => !w.id.startsWith('Shared'));
  await writeFile(state, JSON.stringify(snapshot));
  await session.type('r'); await wait(session, '[Alpha]');
  assert.ok(!(await session.screen()).contains('Shared ·'));
  await session.type('q'); assert.equal(await session.waitForExit({timeout:4000}), 0);
  assert.deepEqual(JSON.parse(await readFile(state, 'utf8')), snapshot, 'Dashboard mutated the fixture');
  console.log('COMPLETE shared TUI: navigation, controls, history, refresh, removal, 5 sizes; normal exit 0.');
} catch (error) {
  for (const session of pilot.sessions()) {
    if (session.exitCode === null) await capture(session, 'failure');
  }
  throw error;
} finally {
  await pilot.close();
  await rm(temporary, {recursive:true, force:true});
}
