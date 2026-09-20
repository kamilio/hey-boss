// Isolated visual fixture: no real agents, discovery or fleet services.
import {copyFileSync, mkdirSync, rmSync} from 'node:fs';
import {spawn, spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
const root = resolve('output/playwright/issue44/runtime');
mkdirSync(root, {recursive:true});
const binary = root+'/hey-boss';
copyFileSync(resolve('target/debug/hey-boss'), binary);
const env = {...process.env, HEY_BOSS_ISSUE_DB:root+'/issues.db', HEY_BOSS_FLEET_STATE:root, HEY_BOSS_INBOX_SOCKET:root+'/inbox.sock', HEY_BOSS_CODEX:'/usr/bin/false'};
delete env.HEY_BOSS_ISSUE_HOST;
for (const title of ['Untouched issue', 'First agent launch', 'Investigate repeated reconnect failures across devices', 'Long-running issue with many worker retries']) {
  const result = spawnSync(binary, ['issue','--json','--project','Launch QA','--agent','human:qa','create','--title',title,'--body','Review worker agent launches.'], {env,encoding:'utf8'});
  if (result.status !== 0) throw Error(result.stderr);
  if (JSON.parse(result.stdout).issue.agent_launch_count !== 0) throw Error('Rebuild hey-boss with launch counting before running this fixture');
}
const seed = spawnSync('sqlite3',[env.HEY_BOSS_ISSUE_DB, `WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1234)
  INSERT INTO issue_agent_launches SELECT 'fixture-'||issues.number||'-'||n.x,project_id,number,created_at FROM issues JOIN n ON n.x<=CASE number WHEN 2 THEN 1 WHEN 3 THEN 12 WHEN 4 THEN 1234 ELSE 0 END;`],{encoding:'utf8'});
if (seed.status !== 0) throw Error(seed.stderr);
const web = spawn(binary, ['issue','--project','Launch QA','web','--port','4794','--no-discovery','--json'], {env,stdio:['ignore','inherit','inherit']});
for (const signal of ['SIGTERM','SIGINT']) process.on(signal, () => web.kill(signal));
web.on('exit', code => {rmSync(root,{recursive:true,force:true});process.exit(code ?? 0);});
