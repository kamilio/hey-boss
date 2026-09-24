// Real terminal interaction; the backend is synthetic and cannot touch live workers.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, readFile, writeFile, rm} from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {TerminalPilot} from 'terminal-pilot';
import {renderTerminalPng} from 'terminal-png';
const root=fileURLToPath(new URL('../',import.meta.url));
const temporary=await mkdtemp(path.join(os.tmpdir(),'hb-project-tabs-'));
const output=path.join(root,'target/project-tabs-qa');
await mkdir(output,{recursive:true});
const statePath=path.join(temporary,'state.json');
const fixture=path.join(temporary,'backend.mjs');
const worker=(id,project)=>({id,pid:42,active:1,config:{name:id,enabled:true,concurrency:1,directory:`/work/${project}`,projects:[`named:${project}`]},runs:[{id:`run-${id}`,project_id:`named:${project}`,project_name:project,number:1,title:`${project} agent`,state:'running',finished_at:null}],chiefs:[]});
await writeFile(statePath,JSON.stringify({workers:[worker('Alpha checkout','Alpha'),worker('Beta checkout','Beta')],mutations:[]}));
await writeFile(fixture,`#!/usr/bin/env node
import {readFileSync,writeFileSync} from 'node:fs';
const file=${JSON.stringify(statePath)};const data=JSON.parse(readFileSync(file,'utf8'));const args=process.argv.slice(2);const action=args[2];
if(action==='add'){
 const name=args[args.indexOf('--name')+1],slots=Number(args[args.indexOf('--concurrency')+1]);
 const directories=args.flatMap((v,i)=>v==='--directory'?[args[i+1]]:[]);
 data.mutations.push({action,name,slots,directories});
 data.workers.push({id:name,pid:42,active:0,config:{name,enabled:true,concurrency:slots,projects:['named:Alpha'],directory:directories.join(' · ')},runs:[],chiefs:[]});
 writeFileSync(file,JSON.stringify(data));console.log(JSON.stringify({ok:true}));
}else if(action==='remove'){
 data.mutations.push({action,id:args[3]});const worker=data.workers.find(w=>w.id===args[3]);worker.intent='drain';worker.config.enabled=false;
 writeFileSync(file,JSON.stringify(data));console.log(JSON.stringify({ok:true}));
}else{console.log(JSON.stringify({ok:true,project_tabs:true,workers:data.workers,store:{host:'fixture'},fleet:{supervisor_connection:{state:'local'}}}));}
`,{mode:0o700});
const pilot=await TerminalPilot.launch();
const wait=async(session,text)=>session.waitFor(text,{scope:'screen',timeout:12000});
const capture=async(session,name)=>{await session.waitForQuiet(100);const screen=await session.screen();await writeFile(path.join(output,name+'.txt'),screen.text);await renderTerminalPng(screen.rawLines.join('\n'),{output:path.join(output,name+'.png')});};
const current=async()=>JSON.parse(await readFile(statePath,'utf8'));
async function open(session,key,title){const deadline=Date.now()+12000;while(Date.now()<deadline){await session.type(key);try{await session.waitFor(title,{scope:'screen',timeout:300});return;}catch{}}throw Error('Could not open '+title);}
try{
 const session=await pilot.newSession({command:path.join(root,'target/debug/hey-boss-worker-tui'),args:['--projects','--binary',fixture],cwd:root,cols:110,rows:30,env:{...process.env,TERM:'xterm-256color'}});
 await wait(session,'Alpha agent');
 await session.press('Tab');await wait(session,'Beta agent');assert.ok(!(await session.screen()).contains('Alpha agent'));
 await session.send('\x1b[Z');await wait(session,'Alpha agent');
 await capture(session,'projects');
 await open(session,'a','Add worker');
 await session.type('Quiet tools');await session.press('Tab');await session.press('Backspace');await session.type('2');await session.press('Tab');await session.type('/work/Tool One');
 await session.press('Control+n');await session.type('/work/Tool Two');await capture(session,'add-worker');await session.press('Enter');
 await wait(session,'Alpha agent');
 await open(session,'w','Project workers');await wait(session,'Quiet tools');
 assert.deepEqual((await current()).mutations[0],{action:'add',name:'Quiet tools',slots:2,directories:['/work/Tool One','/work/Tool Two']});
 await open(session,'d','Remove worker?');await session.press('Escape');assert.equal((await current()).mutations.length,1);
 await open(session,'d','Remove worker?');await session.press('Enter');await wait(session,'Removing after completion');
 assert.equal((await current()).workers[0].active,1,'Removal prematurely ended its agent');
 await capture(session,'draining');
 await session.press('Escape');await wait(session,'Alpha agent');
 const completed=await current();completed.workers.shift();await writeFile(statePath,JSON.stringify(completed));
 await session.type('r');await wait(session,'No active agents');
 await session.resize(48,16);await wait(session,'Shift+Tab');await capture(session,'narrow');
 await session.type('q');assert.equal(await session.waitForExit({timeout:4000}),0);
 assert.equal((await current()).workers.length,2,'Quitting changed worker lifecycle');
 console.log('Project tabs, add worker, graceful remove and narrow layout passed. Screenshots: '+output);
}finally{await pilot.close();await rm(temporary,{recursive:true,force:true});}
