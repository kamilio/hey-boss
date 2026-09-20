// Isolated Chief settings review. No real agent or fleet services are launched.
import {mkdirSync, rmSync} from 'node:fs';
import {spawn, spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
const root = resolve('output/playwright/issue43/runtime');
mkdirSync(root, {recursive:true});
const binary = resolve('target/debug/hey-boss');
const env = {...process.env, HEY_BOSS_ISSUE_DB:root+'/issues.db', HEY_BOSS_FLEET_STATE:root, HEY_BOSS_INBOX_SOCKET:root+'/inbox.sock', HEY_BOSS_CODEX:'/usr/bin/false'};
delete env.HEY_BOSS_ISSUE_HOST;
for (const project of ['Chief QA','Other QA']) {
  const result = spawnSync(binary, ['issue','--project',project,'--agent','human:qa','create','--title','Organize the release'], {env,encoding:'utf8'});
  if (result.status !== 0) throw Error(result.stderr);
}
const web = spawn(binary, ['issue','--project','Chief QA','web','--port','4793','--no-discovery','--json'], {env,stdio:['ignore','inherit','inherit']});
for (const signal of ['SIGTERM','SIGINT']) process.on(signal, () => web.kill(signal));
web.on('exit', code => { rmSync(root,{recursive:true,force:true}); process.exit(code ?? 0); });
