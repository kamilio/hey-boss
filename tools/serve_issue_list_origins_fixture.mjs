// Isolated fixture; never reads or mutates the user's issue database.
import {copyFileSync, mkdirSync, rmSync} from 'node:fs';
import {spawn, spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
const root = resolve('output/playwright/issue76/runtime');
mkdirSync(root, {recursive:true});
const binary = root+'/hey-boss';
copyFileSync(resolve('target/debug/hey-boss'), binary);
const env = {...process.env, HEY_BOSS_ISSUE_DB:root+'/issues.db', HEY_BOSS_FLEET_STATE:root, HEY_BOSS_INBOX_SOCKET:root+'/inbox.sock'};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_AGENT_ID','CODEX_THREAD_ID']) delete env[key];
function issue(args) {
  const result = spawnSync(binary, ['issue','--project','List origin QA','--agent','human:boss','--json',...args], {env,encoding:'utf8'});
  if (result.status) throw Error(result.stderr || result.stdout);
  return JSON.parse(result.stdout);
}
for (const title of ['Legacy issue with no creation context','Populated origin with two comments','Closed legacy issue','Malformed origin keeps its issue visible','Unsupported origin keeps its issue visible']) {
  issue(['create','--title',title,'--body','Historical issue content remains available.','--label','regression']);
}
issue(['comment','2','--body','First finding']);
issue(['comment','2','--body','Second finding']);
issue(['assign-to-boss','2']);
issue(['close','3']);
const seed = spawnSync('sqlite3', [env.HEY_BOSS_ISSUE_DB, `
  UPDATE issues SET origin=NULL WHERE number IN (1,3);
  DROP INDEX issues_origin_session; DROP INDEX issues_origin_run;
  PRAGMA ignore_check_constraints=ON;
  UPDATE issues SET origin='{broken' WHERE number=4;
  UPDATE issues SET origin='[1,2]' WHERE number=5;`], {encoding:'utf8'});
if (seed.status) throw Error(seed.stderr);
const web = spawn(binary, ['issue','--project','List origin QA','web','--port','59676','--no-discovery','--json'], {env,stdio:['ignore','inherit','inherit']});
for (const signal of ['SIGTERM','SIGINT']) process.on(signal, () => web.kill(signal));
web.on('exit', code => {rmSync(root,{recursive:true,force:true});process.exit(code ?? 0);});
