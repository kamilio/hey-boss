// Disposable installed-asset verification. Leave state for cleanup after the delivery comment.
import {mkdtempSync} from 'node:fs';
import {join,resolve} from 'node:path';
import {spawn,spawnSync} from 'node:child_process';
const binary=resolve(process.argv[2]),expected=process.argv[3]||'',root=mkdtempSync('/tmp/hey-boss-issue82-check-');
let web;
try {
 const env={...process.env,HEY_BOSS_AGENT_ID:'human:verification',HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:join(root,'inbox.sock')};
 for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_STATE_DIR'])delete env[key];
 const run=args=>{const r=spawnSync(binary,args,{env,cwd:root,encoding:'utf8'});if(r.status!==0)throw Error(r.stderr||r.stdout);return r.stdout;};
 const version=run(['--version']).trim();if(!version.includes(expected))throw Error('Installed build mismatch');
 run(['issue','--project','Steering verification','create','--title','Verify installed steering']);
 web=spawn(binary,['issue','web','--port','0','--no-discovery','--project','Steering verification','--json'],{env,cwd:root,stdio:['ignore','pipe','pipe']});
 const info=await new Promise((yes,no)=>{let line='';const timer=setTimeout(()=>no(Error('Web startup timed out')),30000);web.stdout.on('data',chunk=>{line+=chunk;if(line.includes('\n')){clearTimeout(timer);try{yes(JSON.parse(line.split('\n')[0]));}catch(e){no(e);}}});web.on('error',no);web.on('exit',()=>no(Error('Web exited')));});
 const base=info.url.replace(/\/$/,'');
 const assets=await Promise.all(['/agents/session','/fleet.js','/fleet.css'].map(async path=>{const r=await fetch(base+path);if(!r.ok)throw Error('Missing installed asset '+path);return r.text();}));
 for(const scope of ['session','issue','project'])if(!assets[0].includes('value="'+scope+'"'))throw Error('Missing installed scope '+scope);
 if(!assets[0].includes('id="steer-open"')||!assets[0].includes('id="steering-updates"')||!assets[1].includes('/api/fleet/steer')||!assets[1].includes('steerRequest.id')||!assets[2].includes('.steer-dialog'))throw Error('Installed steering UI is outdated');
 const blocked=await fetch(base+'/api/fleet/steer',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'});if(blocked.status!==403)throw Error('Installed steering endpoint does not enforce CSRF');
 console.log(JSON.stringify({status:'passed',version,scopes:['session','issue','project'],receipts:true,csrf:true,cleanup_root:root}));
} finally {
 if(web&&web.exitCode===null){const ended=new Promise(r=>web.once('exit',r));web.kill();await ended;}
}
