// Isolated Plan prompt QA; never reads or changes the real issue store.
import {mkdirSync,mkdtempSync,rmSync,copyFileSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
const output=resolve('output/playwright/issue93');mkdirSync(output,{recursive:true});
const root=mkdtempSync(output+'/runtime-'),binary=root+'/hey-boss';
// Keep the fixture stable while other sessions rebuild the shared checkout.
copyFileSync(resolve(process.argv[2]||'target/debug/hey-boss'),binary);
const env={...process.env,HEY_BOSS_ISSUE_DB:root+'/issues.db',HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:root+'/inbox.sock',HEY_BOSS_AGENT_ID:'human:qa'};
for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID'])delete env[key];
const cli=args=>{const r=spawnSync(binary,['issue','--project','Plan QA',...args],{env,encoding:'utf8'});if(r.status)throw Error(r.stdout+r.stderr);};
cli(['create','--title','Plan reconnect improvements','--label','task:plan']);
cli(['create','--title','Legacy artifact task','--label','task:research']);
const web=spawn(binary,['issue','--project','Plan QA','web','--port','59693','--no-discovery','--json'],{env,stdio:['ignore','inherit','inherit']});
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,()=>web.kill());
web.on('exit',()=>rmSync(root,{recursive:true,force:true}));
