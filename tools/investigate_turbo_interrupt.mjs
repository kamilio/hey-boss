// Manual, isolated reproduction for issue 135. Pass an installed native Turbo
// executable; no downloads, project tasks, commits, or general validation gate.
import assert from 'node:assert/strict';
import {spawn, spawnSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';

assert(process.argv[2], 'Usage: node tools/investigate_turbo_interrupt.mjs /path/to/native/turbo');
const turbo = resolve(process.argv[2]);
const env = {...process.env, CI: '1', TURBO_TELEMETRY_DISABLED: '1'};
const version = spawnSync(turbo, ['--version'], {env, encoding: 'utf8', timeout: 5000});
assert.equal(version.status, 0, version.stderr);
console.log(JSON.stringify({version: version.stdout.trim(), sha256: createHash('sha256').update(readFileSync(turbo)).digest('hex')}));
const root = mkdtempSync(join(tmpdir(), 'hey-boss-135-turbo-'));
let child;
const killGroup = signal => {
  if (!child?.pid) return;
  try { process.kill(-child.pid, signal); } catch (error) { if (error.code !== 'ESRCH') throw error; }
};
const abort = () => { killGroup('SIGKILL'); process.exitCode = 1; };
process.on('SIGINT', abort);
process.on('SIGTERM', abort);
try {
  writeFileSync(join(root, 'package.json'), JSON.stringify({name: 'interruption-fixture', private: true, packageManager: 'npm@10.9.2', workspaces: ['packages/*']}));
  writeFileSync(join(root, 'package-lock.json'), JSON.stringify({name: 'interruption-fixture', lockfileVersion: 3, packages: {'': {name: 'interruption-fixture', workspaces: ['packages/*']}, 'packages/fast': {name: 'fast', version: '1.0.0'}, 'packages/slow': {name: 'slow', version: '1.0.0'}}}));
  writeFileSync(join(root, 'turbo.json'), JSON.stringify({tasks: {test: {cache: false}}}));
  for (const name of ['fast', 'slow']) {
    const path = join(root, 'packages', name);
    mkdirSync(path, {recursive: true});
    writeFileSync(join(path, 'package.json'), JSON.stringify({name, version: '1.0.0', scripts: {test: 'node test.cjs'}}));
    writeFileSync(join(path, 'test.cjs'), name === 'fast' ? "console.log('FAST_DONE');" : "require('fs').writeFileSync('started','yes'); setTimeout(()=>console.log('SLOW_DONE'),1500);");
  }
  for (const requestedSignal of [null, 'SIGINT', 'SIGTERM']) {
    if (process.exitCode) break;
    const started = join(root, 'packages/slow/started');
    const runs = join(root, '.turbo/runs');
    rmSync(started, {force: true});
    rmSync(runs, {force: true, recursive: true});
    child = spawn(turbo, ['run', 'test', '--concurrency=1', '--summarize', '--cache=local:'], {cwd: root, env, detached: true, stdio: ['ignore', 'pipe', 'pipe']});
    let output = '', sent = false, timedOut = false;
    for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => { output += chunk; });
    const interrupt = setInterval(() => {
      if (requestedSignal && !sent && existsSync(started)) {
        sent = true;
        killGroup(requestedSignal);
      }
    }, 20);
    const timeout = setTimeout(() => { timedOut = true; killGroup('SIGKILL'); }, 15000);
    let result;
    try {
      result = await new Promise((resolve, reject) => {
        child.once('error', reject);
        child.once('close', (code, signal) => resolve({code, signal}));
      });
    } finally {
      clearInterval(interrupt);
      clearTimeout(timeout);
      killGroup('SIGKILL'); // Also remove any surviving fixture descendants.
      child = undefined;
    }
    const reports = existsSync(runs) ? readdirSync(runs).filter(name => name.endsWith('.json')).map(name => JSON.parse(readFileSync(join(runs, name), 'utf8'))) : [];
    console.log(JSON.stringify({requestedSignal, sent, timedOut, ...result, output, reports: reports.map(report => ({execution: report.execution, tasks: report.tasks.map(task => ({taskId: task.taskId, execution: task.execution}))}))}));
    assert(!timedOut, 'Fixture timed out');
    if (process.exitCode) break;
    if (requestedSignal) assert(sent, 'Slow task never started; interruption was not tested');
    else {
      assert.equal(result.code, 0, 'Uninterrupted control failed');
      assert.equal(reports.length, 1, 'Control needs one fresh summary');
      assert.equal(reports[0].execution.success, 2, 'Control did not finish both tasks');
      assert.deepEqual(reports[0].tasks.map(task => task.taskId).sort(), ['fast#test', 'slow#test']);
    }
  }
} finally {
  killGroup('SIGKILL');
  rmSync(root, {recursive: true, force: true});
  process.off('SIGINT', abort);
  process.off('SIGTERM', abort);
}
