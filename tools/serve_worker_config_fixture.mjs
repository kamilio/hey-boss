// Isolated supervisor and native HTTP server for worker configuration checks.
import {mkdirSync,writeFileSync,existsSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
const root=resolve('output/playwright/auto-workers/fixture');
const binary=resolve(process.env.HEY_BOSS_TEST_BINARY||'target/auto-workers/debug/hey-boss');
mkdirSync(root,{recursive:true});
const desired=root+'/fleet.yaml';
writeFileSync(desired,'# All worker instances\nmachines:\n  local:\n    workers:\n      - id: tools\n        intent: pause\n        config:\n          name: Tools\n          concurrency: 2\n');
const env={...process.env,HEY_BOSS_ISSUE_DB:root+'/issues.db',HEY_BOSS_FLEET_STATE:root,HEY_BOSS_FLEET_DESIRED:desired,HEY_BOSS_FLEET_BINARY:binary};
delete env.HEY_BOSS_ISSUE_HOST;delete env.HEY_BOSS_ISSUE_PROJECT;
const children=[];
function child(args){const process=spawn(binary,args,{env,stdio:['ignore','inherit','inherit']});children.push(process);return process;}
const supervisor=child(['fleet','supervisor']);
for(let n=0;n<100&&!existsSync(root+'/fleet.sock');n++)await new Promise(r=>setTimeout(r,100));
if(!existsSync(root+'/fleet.sock'))throw Error('Fixture supervisor did not start');
const project=spawnSync(binary,['issue','--project','Atlas','create','--title','Synthetic worker configuration check','--json'],{env,encoding:'utf8'});
if(project.status!==0)throw Error(project.stderr);
const web=child(['issue','web','--port','59648','--no-discovery','--project','Atlas','--json']);
let closing=false;
function close(){if(closing)return;closing=true;for(const process of children)process.kill('SIGTERM');}
for(const signal of ['SIGINT','SIGTERM'])process.on(signal,close);
supervisor.on('exit',close);web.on('exit',close);
