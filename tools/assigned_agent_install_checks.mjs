// Verify the installed executable's embedded navigation UI using disposable state.
import {mkdtempSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawn,spawnSync} from 'node:child_process';
const binary=resolve(process.argv[2]), expected=process.argv[3]||'';
const root=mkdtempSync(join(tmpdir(),'hey-boss-assignment-check-'));let web;
try {
  const env={...process.env,HEY_BOSS_AGENT_ID:'human:verification',HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:join(root,'inbox.sock')};
  for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_STATE_DIR'])delete env[key];
  const run=args=>{const r=spawnSync(binary,args,{env,cwd:root,encoding:'utf8'});if(r.status!==0)throw Error(r.stderr||r.stdout);return r.stdout;};
  const version=run(['--version']).trim();if(!version.includes(expected))throw Error('Installed build mismatch');
  run(['issue','--project','Assignment verification','create','--title','Verify installed navigation']);
  web=spawn(binary,['issue','web','--port','0','--no-discovery','--project','Assignment verification','--json'],{env,cwd:root,stdio:['ignore','pipe','pipe']});
  const info=await new Promise((yes,no)=>{let line='';const timer=setTimeout(()=>no(Error('Web startup timed out')),20000);web.stdout.on('data',chunk=>{line+=chunk;if(line.includes('\n')){clearTimeout(timer);try{yes(JSON.parse(line.split('\n')[0]));}catch(e){no(e);}}});web.on('error',no);web.on('exit',()=>no(Error('Web exited')));});
  const base=info.url.replace(/\/$/,'');
  const assets=await Promise.all(['/app.js','/app.css','/fleet.js'].map(async path=>{const r=await fetch(base+path);if(!r.ok)throw Error('Missing installed asset '+path);return r.text();}));
  if(!assets[0].includes('list-agent-trace')||!assets[0].includes('listAssignee(i.assignee, i.number)')||!assets[1].includes('.list-agent-trace')||!assets[2].includes('function assignedAgentEntry')||!assets[2].includes('history.replaceState'))throw Error('Installed assignment navigation is outdated');
  console.log(JSON.stringify({status:'passed',version,installed_assignment_navigation:true}));
} finally {
  if(web&&web.exitCode===null){const ended=new Promise(r=>web.once('exit',r));web.kill();await ended;}
  rmSync(root,{recursive:true,force:true});
}
