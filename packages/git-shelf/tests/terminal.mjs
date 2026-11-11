// Run after npm ci in worker-tui. Optional final argument retains screen PNGs.
import { TerminalPilot } from '../../../worker-tui/node_modules/terminal-pilot/dist/index.js';
import { renderTerminalPng } from '../../../worker-tui/node_modules/terminal-png/dist/index.js';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, symlinkSync, writeFileSync, rmSync } from 'node:fs';
import { join, resolve } from 'node:path';
import assert from 'node:assert/strict';
const [g,hb,output]=process.argv.slice(2).map(p=>resolve(p));
const root=mkdtempSync(join(process.cwd(),'.shelf-ui-')), cwd=join(root,'repo');
mkdirSync(cwd);mkdirSync(join(root,'bin'));symlinkSync(hb,join(root,'bin','hey-boss'));
const env={...process.env,TERM:'xterm-256color',PATH:join(root,'bin')+':'+process.env.PATH,HEY_BOSS_ISSUE_DB:join(root,'db'),HEY_BOSS_FLEET_STATE:join(root,'fleet'),HEY_BOSS_AGENT_ID:'human:shelf-test',GIT_CONFIG_NOSYSTEM:'1',GIT_CONFIG_GLOBAL:'/dev/null'};
env.NO_COLOR="";env.CLICOLOR_FORCE="1";delete env.HEY_BOSS_ISSUE_HOST;delete env.HEY_BOSS_ISSUE_PROJECT;
const run=(cmd,...args)=>execFileSync(cmd,args,{cwd,env,encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim();
const pilot=await TerminalPilot.launch();
try{
  run('git','init','-qb','main');run('git','config','user.name','Sam');run('git','config','user.email','shelf@example.invalid');
  run('git','remote','add','origin','https://github.com/example/catalog.git');
  run(hb,'project','init','--yes','--prs','false','--worktree','true');
  run(hb,'issue','create','--title','Tune the search timeout');
  writeFileSync(join(cwd,'search.rs'),'const TIMEOUT_MS: u32 = 100;\n');
  run('git','add','.');run('git','commit','-qm','Initialize search');
  const session=await pilot.newSession({command:g,cwd,env,cols:106,rows:27});
  const wait=s=>session.waitFor(s,{scope:'screen',timeout:10000});
  await wait('No shelved commits yet');
  await session.press('Escape');assert.equal(await session.waitForExit(),0);
  for(const [name,value] of [['Allow slower connections',200],['Increase search timeout',250]]){
    writeFileSync(join(cwd,'search.rs'),`const TIMEOUT_MS: u32 = ${value};\n`);
    run('git','commit','-qam',name);
    if(value===200){run(g,'stash');continue;}
  }
  const s=await pilot.newSession({command:g,cwd,env,cols:106,rows:27});
  const see=t=>s.waitFor(t,{scope:'screen',timeout:10000});
  const shot=async name=>{await s.waitForQuiet(120);if(output){mkdirSync(output,{recursive:true});await renderTerminalPng((await s.screen()).rawLines.join('\n'),{output:join(output,name+'.png')});}};
  await see('Allow slower connections');await s.press('Control+s');await see('recommended');await s.type('1');await s.press('Enter');await see('Shelved');
  // Refresh preserves the existing selection. Select the newly saved commit.
  await s.press('ArrowUp');await s.waitForQuiet(150);if(!(await s.screen()).contains('250;'))await s.press('ArrowDown');await see('250;');await s.press('Enter');await s.press('PageDown');
  await s.press('Control+c');await see('Comment on the selected line');await s.type('Matches the mobile timeout.');await s.press('Enter');await see('Comment saved');
  await s.press('Tab');await see('Matches the mobile timeout.');await shot('project-shelf');
  await s.resize(48,23);await s.press('ArrowLeft');await see('Ctrl+L link');assert(!(await s.screen()).contains('Review'));await shot('compact-shelf');
  await s.press('Enter');await see('Matches the mobile timeout.');await shot('compact-review');
  await s.press('Escape');await see('Commits');await s.press('Control+u');await see('Cherry-pick selected');await s.press('Escape');
  assert.equal(run('git','log','-1','--format=%s'),'Initialize search');
  await s.press('Escape');assert.equal(await s.waitForExit(),0);
  console.log('Terminal Pilot: empty shelf, task-linked stash, inline comment, navigation, resize and canceled restore passed.');
}finally{await pilot.close();rmSync(root,{recursive:true,force:true});}
