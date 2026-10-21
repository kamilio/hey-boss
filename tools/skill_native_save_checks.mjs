// Real native-open payload and Rust save/distribution protocol, with disposable homes.
import assert from 'node:assert/strict';
import {mkdtemp,mkdir,writeFile,readFile,rm,chmod,readdir} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawn} from 'node:child_process';
import {createServer} from 'node:net';
const binary=resolve(process.env.HEY_BOSS_TEST_BINARY||'target/debug/hey-boss');
const root=await mkdtemp(join(tmpdir(),'skill-native-'));
const local=join(root,'local'),remote=join(root,'remote');
const text=(name,body='Original')=>`---\nname: ${name}\ndescription: Fixture\n---\n# Example\n${body}\n`;
const put=async(p,s)=>{await mkdir(resolve(p,'..'),{recursive:true});await writeFile(p,s);};
const calls=[];let web,log='',origin,csrf;
const desktop=createServer({allowHalfOpen:true},socket=>{let body='';socket.on('data',c=>body+=c);socket.on('end',()=>{calls.push(JSON.parse(JSON.parse(body).question));socket.end(JSON.stringify({status:'ok',result:'{"ok":true}'}));});});
const cli=(args,input)=>new Promise(done=>{const child=spawn(binary,args,{cwd:local,env,stdio:['pipe','pipe','pipe']});let out='',err='';child.stdout.on('data',c=>out+=c);child.stderr.on('data',c=>err+=c);child.on('exit',code=>done({code,out,err}));child.stdin.end(input?JSON.stringify(input):'');});
const env={...process.env,HOME:local,PATH:`${join(root,'bin')}:${process.env.PATH}`,HEY_BOSS_STATE_DIR:join(root,'state'),HEY_BOSS_INBOX_SOCKET:join(root,'desktop.sock')};
for(const key of ['HEY_BOSS_FLEET_CONFIG','HEY_BOSS_FLEET_STATE','HEY_BOSS_FLEET_DESIRED','HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_DB'])delete env[key];
try{
 await new Promise(r=>desktop.listen(env.HEY_BOSS_INBOX_SOCKET,r));
 for(const home of [local,remote]){
  await put(join(home,'.codex/skills/example/SKILL.md'),text('example'));
  await put(join(home,'.codex/skills/example/references/notes.md'),'# Notes\n');
 }
 await put(join(local,'.hey-boss/config.json'),JSON.stringify({ssh_hosts:['fixture-remote']}));
 await put(join(root,'bin/ssh'),`#!/usr/bin/env node\nconst {spawnSync}=require('node:child_process'),{existsSync}=require('node:fs');if(existsSync(${JSON.stringify(join(root,'offline'))})){console.error('Fixture offline');process.exit(1);}const cmd=process.argv.at(-1);const r=cmd.includes('skill apply-')?spawnSync(${JSON.stringify(binary)},['skill',cmd.includes('apply-copy')?'apply-copy':'apply-rename'],{stdio:'inherit',env:{...process.env,HOME:${JSON.stringify(remote)}}}):spawnSync('python3',[${JSON.stringify(resolve('src/skill/transport.py'))}],{stdio:'inherit',env:{...process.env,HOME:${JSON.stringify(remote)}}});process.exit(r.status??1);\n`);
 await chmod(join(root,'bin/ssh'),0o755);
 web=spawn(binary,['issue','web','--port','0','--no-discovery','--json'],{cwd:local,env});web.stdout.on('data',c=>log+=c);web.stderr.on('data',c=>log+=c);
 for(let n=0;n<300&&!origin;n++){for(const line of log.split('\n'))try{origin=JSON.parse(line).url||origin;}catch{}if(web.exitCode!==null)throw Error(log);if(!origin)await new Promise(r=>setTimeout(r,100));}
 assert.ok(origin,log);origin=origin.replace(/\/$/,'');
 const get=async()=>{const r=await fetch(origin+'/api/skills');return r.json();};
 csrf=(await (await fetch(origin+'/api/bootstrap')).json()).csrf;
 const post=async body=>{const res=await fetch(origin+'/api/skills',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':csrf},body:JSON.stringify(body)});const result=await res.json();assert.equal(res.ok,true,JSON.stringify(result));return result;};
 const idle=async()=>{for(let n=0;n<300;n++){const r=await get();if(!r.busy)return r;await new Promise(r=>setTimeout(r,100));}throw Error('Busy');};
 await post({action:'scan'});await idle();
 await post({action:'open_editor',skill:'example',file_path:'SKILL.md'});
 const launch=calls.at(-1);assert.ok(launch.skillSession);assert.equal(await readFile(launch.file,'utf8'),text('example'));
 assert.equal(await readFile(join(resolve(launch.file,'..'),'references/notes.md'),'utf8'),'# Notes\n');
 await post({action:'open_editor',skill:'example',file_path:'references/notes.md'});const reference=calls.at(-1);
 const save=async(launch,content,id,success=true)=>{const result=await cli(['skill','--json','editor-save',launch.skillSession],{content,request_id:id});assert.equal(result.code===0,success,result.out+result.err);return JSON.parse(result.out);};
 const verify=async(name,content)=>{for(const home of [local,remote])for(const agent of ['.codex','.agents','.claude','.hey-boss'])assert.equal(await readFile(join(home,agent,'skills',name,'SKILL.md'),'utf8'),content);};
 await put(join(remote,'.codex/skills/example/SKILL.md'),text('example','Changed after scanning'));
 await save(launch,text('example','Saved /tmp/path'),'first');await verify('example',text('example','Saved /tmp/path'));
 // A separate reference editor keeps newer changes in the primary document.
 await save(reference,'# Reference edited\n','reference');await verify('example',text('example','Saved /tmp/path'));
 await save(launch,text('example','Second save'),'second');await verify('example',text('example','Second save'));
 for(const home of [local,remote])assert.equal(await readFile(join(home,'.codex/skills/example/references/notes.md'),'utf8'),'# Reference edited\n');
 // A delayed reply for an earlier save must not undo a later save.
 await save(launch,text('example','Saved /tmp/path'),'first');await verify('example',text('example','Second save'));
 const history=await readdir(join(remote,'.hey-boss/skill-backups'));
 assert.ok((await Promise.all(history.map(p=>readFile(join(remote,'.hey-boss/skill-backups',p,'.codex/skills/example/SKILL.md'),'utf8').catch(()=>'')))).some(s=>s.includes('Changed after scanning')));
 // Offline writes remain retryable, including after a fresh CLI process starts.
 const journal=join(local,'.hey-boss/skill-edits',launch.skillSession,'session.json');
 const beforeRename=await readFile(journal);
 await put(join(root,'offline'),'');
 const pending=await save(launch,text('renamed','Offline rename'),'rename',false);assert.equal(pending.error.code,'sync_pending');
 // Simulate interruption between committed rename and editing-session bookkeeping.
 await writeFile(journal,beforeRename);
 await rm(join(root,'offline'));
 await save(launch,text('renamed','Offline rename'),'rename');await verify('renamed',text('renamed','Offline rename'));
 await save(launch,text('renamed','Next save after rename'),'after-rename');await verify('renamed',text('renamed','Next save after rename'));
 // The same native file stays valid after the installed folder is renamed.
 assert.equal(await readFile(launch.file,'utf8'),text('renamed','Next save after rename'));
 if(process.env.SKILL_NATIVE_AUDIT){
  await put(join(root,'launch.json'),JSON.stringify(launch));
  const result=await new Promise(done=>{const child=spawn(process.env.SKILL_NATIVE_AUDIT,[],{env:{...env,HEY_BOSS_CLI_PATH:binary,HEY_BOSS_AUDIT_SKILL_EDITOR:join(root,'launch.json'),SKILLS_SCREENSHOTS:process.env.SKILLS_SCREENSHOTS||''},stdio:['ignore','pipe','pipe']});let out='';child.stdout.on('data',c=>out+=c);child.stderr.on('data',c=>out+=c);child.on('exit',code=>done({code,out}));});assert.equal(result.code,0,result.out);console.log(result.out);
  await verify('renamed',text('renamed','Next save after rename')+'\nNative / edit 🦀\n');
 }
 console.log('PASS native opening, cross-file saves, drift overwrite, backups, delayed retries, offline rename and subsequent saves');
}finally{web?.kill();if(web&&web.exitCode===null)await new Promise(r=>web.once('exit',r));await new Promise(r=>desktop.close(r));await rm(root,{recursive:true,force:true});}
