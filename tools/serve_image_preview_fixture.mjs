// Isolated native issue UI. Stop with Ctrl+C to remove the fixture.
import {mkdtempSync, rmSync, existsSync, realpathSync, copyFileSync} from 'node:fs';
import {spawn} from 'node:child_process';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {createHash} from 'node:crypto';

const root = realpathSync(mkdtempSync(join(tmpdir(), 'hey-boss-image-preview-')));
const database = join(root, 'issues.db');
const env = {...process.env, HEY_BOSS_ISSUE_DB:database, HEY_BOSS_FLEET_STATE:root,
  HEY_BOSS_INBOX_SOCKET:join(root, 'inbox.sock'), HEY_BOSS_AGENT_ID:'human:boss'};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID']) delete env[key];
// Keep concurrent checkout builds from restarting the fixture midway through a check.
const binary = join(root, 'hey-boss');
copyFileSync(resolve(process.argv[2] || 'target/debug/hey-boss'), binary);
const children = [];
const start = args => {
  const child = spawn(binary, args, {env, stdio:['ignore','inherit','inherit']});
  children.push(child);
  return child;
};
const owner = start(['fleet','companion']);
const identity = createHash('sha256').update(database).digest('hex').slice(0,24);
const socketBase = `/tmp/hey-boss-db-${process.getuid()}/${identity}`;
let closing = false;
async function close() {
  if (closing) return;
  closing = true;
  await Promise.all(children.map(child => new Promise(resolve => {
    if (child.exitCode !== null || child.signalCode !== null) return resolve();
    child.once('exit', resolve);
    child.kill('SIGTERM');
  })));
  rmSync(root, {recursive:true,force:true});
  for (const suffix of ['.sock','.lock','.startup','.log']) rmSync(socketBase+suffix,{force:true});
  console.log('COMPLETE: fixture processes stopped and temporary data removed');
}
for (const signal of ['SIGTERM','SIGINT']) process.on(signal, close);
try {
  for (let attempt=0; !existsSync(socketBase+'.sock'); attempt++) {
    if (attempt === 200 || owner.exitCode !== null) throw Error('Fixture database did not start');
    await new Promise(resolve=>setTimeout(resolve,50));
  }
  const command = args => new Promise((resolve, reject) => {
    const child = spawn(binary, args, {env, cwd:root, stdio:['ignore','pipe','pipe']});
    let error='';child.stderr.on('data',data=>error+=data);child.stdout.resume();
    child.on('error',reject);child.on('exit',code=>code===0?resolve():reject(Error(error||'Fixture initialization failed')));
  });
  await command(['project','init','--project','Image Preview QA','--yes','--prs','false','--worktree','false','--json']);
  start(['issue','--project','Image Preview QA','web','--port','59661','--no-discovery','--json']);
  const native='http://127.0.0.1:59661';
  let boot;
  for (let attempt=0; !boot; attempt++) {
    if (attempt === 200) throw Error('Fixture web server did not start');
    try {boot=await (await fetch(native+'/api/bootstrap')).json();}
    catch {await new Promise(resolve=>setTimeout(resolve,50));}
  }
  console.log(JSON.stringify({fixture_pid:process.pid,owner_pid:owner.pid,native}));
} catch(error) {console.error(error);process.exitCode=1;await close();}
