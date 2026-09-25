// Real terminal, synthetic machine state, no maintenance or SSH side effects.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, writeFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
import {TerminalPilot} from '../worker-tui/node_modules/terminal-pilot/dist/index.js';
import {renderTerminalPng} from '../worker-tui/node_modules/terminal-png/dist/index.js';
const root=await mkdtemp(join(tmpdir(),'harvester-failure-ui-'));
const output=resolve(process.argv[3]||'output/playwright/issue364');
let pilot;
try {
  await mkdir(output,{recursive:true});await mkdir(join(root,'bin'));
  const snapshot={observed_at:1,last_cleanup_at:1,phase:'Finished with inspection errors',metrics:{disk_path:'/fixture',memory_pressure:'Warning'},config:{},processes:[],worktrees:[
    {name:'/fixture/large-checkout',detail:'Git path inspection failed; checkout retained for retry',error:'Git path inspection failed',eligible:false},
    {name:'/fixture/database-checkout',detail:'SQLite database or sidecar preserved',eligible:false},
  ],harvested_processes:0,removed_worktrees:0,errors:['/fixture/large-checkout: Git path inspection failed'],cache_progress:{visited_this_cycle:90000,slice_millis:25000,roots_pending:12,last_completed_at:100}};
  await writeFile(join(root,'snapshot.json'),JSON.stringify(snapshot));
  await writeFile(join(root,'bin/ssh'),`#!/bin/sh\ncat '${root}/snapshot.json'\n`,{mode:0o700});
  pilot=await TerminalPilot.launch();
  const s=await pilot.newSession({command:resolve(process.argv[2]),args:['--host','fixture.invalid'],cwd:root,cols:100,rows:24,env:{...process.env,HOME:root,HEY_BOSS_HEALTH_DIR:join(root,'health'),PATH:join(root,'bin')+':'+process.env.PATH,TERM:'xterm-256color'}});
  const wait=text=>s.waitFor(text,{scope:'screen',timeout:10000});
  const capture=async name=>{await s.waitForQuiet(150);const screen=await s.screen();await renderTerminalPng(screen.rawLines.join('\n'),{output:join(output,name+'.png')});return screen.text;};
  await wait('Inspection errors: 1');await wait('90000 entries');await capture('overview');
  await s.type('3');await wait('failed /fixture/large-checkout');await wait('preserved /fixture/database-checkout');await capture('worktrees');
  await s.press('Enter');await wait('Selected entry');
  for (const [width,height] of [[100,24],[48,20]]) {
    await s.resize(width,height);await delay(300);await wait('retry');await capture(`failure-details-${width}`);
  }
  await s.press('Escape');await s.type('q');assert.equal(await s.waitForExit({timeout:5000}),0);
  console.log(JSON.stringify({normalCompletion:true,completed:5,expected:5,checks:['Error count visible','Cache throughput visible','Failed and protected worktrees distinct','Failure details readable at wide and narrow widths','Normal exit']}));
} finally {if(pilot)await pilot.close();await rm(root,{recursive:true,force:true});}
