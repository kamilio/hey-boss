// Disposable real-server fixture for hostname/browser and executable-reload QA.
// Stop with SIGTERM; only this fixture's process and files are removed.
import { spawn } from 'node:child_process';
import { copyFileSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import readline from 'node:readline';
import { createServer } from 'node:net';

const root = resolve('output/playwright/issue71/runtime');
mkdirSync(root, { recursive: true });
const binary = `${root}/hey-boss`;
copyFileSync(resolve('target/debug/hey-boss'), binary);
const env = {
  ...process.env,
  HEY_BOSS_ISSUE_DB: `${root}/issues.db`,
  HEY_BOSS_FLEET_STATE: root,
  HEY_BOSS_INBOX_SOCKET: `${root}/inbox.sock`,
};
for (const key of ['HEY_BOSS_ISSUE_HOST', 'HEY_BOSS_ISSUE_PROJECT', 'HEY_BOSS_AGENT_ID', 'CODEX_THREAD_ID']) delete env[key];
// A private empty inbox avoids exposing or modifying desktop notifications.
const inbox = createServer({allowHalfOpen:true}, socket => {
  let request = '';
  socket.on('data', bytes => request += bytes);
  socket.on('end', () => {
    const command = JSON.parse(request).command;
    const reply = command === 'inbox_list'
      ? {status:'ok',result:JSON.stringify({tasks:[],unread:0})}
      : {status:'error',error:'The browser fixture supports only inbox_list'};
    socket.end(JSON.stringify(reply));
  });
});
await new Promise((resolve, reject) => {inbox.once('error',reject);inbox.listen(env.HEY_BOSS_INBOX_SOCKET,resolve);});
const web = spawn(binary, ['issue', 'web', '--port', '0', '--no-discovery', '--project', 'Local HTTP QA', '--json'], { cwd: root, env, stdio: ['ignore', 'pipe', 'inherit'] });
let starts = 0;
readline.createInterface({ input: web.stdout }).on('line', line => {
  const ready = JSON.parse(line);
  starts += 1;
  writeFileSync(`${root}/ready.json`, JSON.stringify({ ...ready, starts, pid: web.pid }));
  console.log(JSON.stringify({ event: 'ready', ...ready, starts, pid: web.pid }));
});
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => web.kill(signal));
web.on('exit', (code, signal) => {
  inbox.close();
  rmSync(root, { recursive: true, force: true });
  console.log(JSON.stringify({ event: 'fixture_stopped', code, signal, complete: code === 0 && signal === null }));
  process.exitCode = code === 0 && signal === null ? 0 : 1;
});
