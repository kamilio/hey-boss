// Exercise the installed artifact commands and embedded UI using disposable state.
import assert from 'node:assert/strict';
import {mkdtempSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawn} from 'node:child_process';

const binary=resolve(process.argv[2]),expected=process.argv[3];
assert.ok(expected,'Supply the expected installed build ID');
const root=mkdtempSync(join(tmpdir(),'hey-boss-artifact-install-'));
const env={...process.env,HEY_BOSS_AGENT_ID:'human:verification',HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:join(root,'inbox.sock')};
for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_STATE_DIR'])delete env[key];
const project='Artifact installation verification',checks=[];
const check=(value,name)=>{assert.ok(value,name);checks.push(name);};
const run=args=>new Promise((yes,no)=>{
  const child=spawn(binary,args,{env,cwd:root,stdio:['ignore','pipe','pipe']});
  let stdout='',stderr='';
  child.stdout.on('data',v=>stdout+=v);child.stderr.on('data',v=>stderr+=v);
  child.on('error',no);child.on('close',(code,signal)=>{
    if(signal||code===null)no(Error(`Incomplete command: ${args.join(' ')} (${signal})`));
    else yes({code,stdout,stderr});
  });
});
const artifact=async args=>{
  const result=await run(['artifact','--project',project,'--json',...args]);
  assert.equal(result.code,0,result.stderr||result.stdout);
  return JSON.parse(result.stdout);
};
let web;
try {
  const version=await run(['--version']);assert.equal(version.code,0,version.stderr);
  check(version.stdout.includes(expected),'Installed build matches');
  // Start the owner before CLI mutations so the fixture leaves no detached broker.
  web=spawn(binary,['issue','web','--port','0','--no-discovery','--project',project,'--json'],{env,cwd:root,stdio:['ignore','pipe','pipe']});
  const info=await new Promise((yes,no)=>{
    let text='',stderr='';const timer=setTimeout(()=>no(Error('Incomplete web startup: timed out')),60000);
    const fail=e=>{clearTimeout(timer);no(e);};
    web.stderr.on('data',v=>stderr+=v);
    web.on('error',fail);web.on('exit',()=>fail(Error('Web exited before startup: '+stderr)));
    web.stdout.on('data',v=>{text+=v;if(text.includes('\n')){clearTimeout(timer);try{yes(JSON.parse(text.split('\n')[0]));}catch(e){no(e);}}});
  });
  const base=info.url.replace(/\/$/,'');
  const assets=await Promise.all(['artifacts.js','artifacts.css','artifacts'].map(async path=>{
    const response=await fetch(base+'/'+path);assert.equal(response.status,200);return response.text();
  }));
  check(assets[0].includes('data-delete=')&&assets[0].includes('function confirmDelete('),'Installed quick actions and confirmation');
  check(assets[1].includes('.artifact-delete-dialog')&&assets[1].includes('width: 44px; height: 44px;'),'Installed responsive actions');
  check(assets[2].includes('id="artifact-archived"'),'Installed archive filter');
  const created=await artifact(['create','--title','Disposable scratch','--body','Delete this test document']);
  const id=created.artifact.id;
  check(created.artifact.version===1,'CLI create');
  await artifact(['archive',id,'--if-version','1']);
  check((await artifact(['list','--archived'])).artifacts.some(a=>a.id===id&&a.archived),'CLI archive');
  await artifact(['restore',id,'--if-version','2']);
  check((await artifact(['view',id])).artifact.version===3,'CLI restore');
  const stale=await run(['artifact','--project',project,'delete',id,'--if-version','1']);
  check(stale.code!==0&&stale.stderr.includes('Artifact changed'),'Stale deletion rejected');
  const args=['delete',id,'--if-version','3','--request-id','verified-delete'];
  const deleted=await artifact(args);
  check(deleted.deleted===id,'CLI permanent deletion');
  check(JSON.stringify(await artifact(args))===JSON.stringify(deleted),'Deletion replay');
  const missing=await run(['artifact','--project',project,'view',id]);
  check(missing.code!==0&&missing.stderr.includes('Artifact not found'),'Deleted document unavailable');
  check(!(await artifact(['list','--archived'])).artifacts.some(a=>a.id===id),'Deleted document absent from archive');
  assert.equal(checks.length,12,'Incomplete installation checks');
  console.log(JSON.stringify({status:'passed',version:version.stdout.trim(),completed:checks.length,expected:12,checks}));
} finally {
  if(web&&web.exitCode===null&&web.signalCode===null){const ended=new Promise(r=>web.once('exit',r));web.kill();await ended;}
  rmSync(root,{recursive:true,force:true});
}
