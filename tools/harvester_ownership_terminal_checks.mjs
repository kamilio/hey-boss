// Real PTY, synthetic remote status, no maintenance mutations or real SSH.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, writeFile, readFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
import {TerminalPilot} from '../worker-tui/node_modules/terminal-pilot/dist/index.js';
import {renderTerminalPng} from '../worker-tui/node_modules/terminal-png/dist/index.js';

const binary=resolve(process.argv[2]||'target/debug/hey-harvester');
const output=resolve('output/playwright/issue147-final');
const root=await mkdtemp(join(tmpdir(),'hb-ownership-terminal-'));
const checks=[];
let pilot;
try {
  await mkdir(output,{recursive:true});
  await mkdir(join(root,'bin'));
  const snapshot={observed_at:1,last_cleanup_at:null,metrics:{disk_path:'/fixture',memory_pressure:'normal'},config:{},processes:[],worktrees:[{
    name:'/Users/example/Workspace/very-long-project-name/active-issue-worktree',
    detail:'Locked worktree; preserved — issue 147; owner fixture-session; queued validation; retain staged changes and receipts',
    eligible:false,worktree:{path:'/fixture/owned',age_seconds:100,repository:'example/project',github_url:null},
  }],harvested_processes:0,removed_worktrees:0,errors:[]};
  for (const detail of ['Missing checkout; metadata preserved', 'Modified, untracked, or ignored files; preserved']) {
    snapshot.worktrees.push({name:'/Users/example/Workspace/recovery-'+snapshot.worktrees.length,
      detail,eligible:false,worktree:{path:'/fixture/recovery',repository:'example/project',github_url:null}});
  }
  await writeFile(join(root,'snapshot.json'),JSON.stringify(snapshot));
  await writeFile(join(root,'bin/ssh'),`#!/bin/sh\nset -eu\nprintf '%s\\n' "$*" >> '${root}/calls'\ncat '${root}/snapshot.json'\n`,{mode:0o700});
  pilot=await TerminalPilot.launch();
  const session=await pilot.newSession({command:binary,args:['--host','fixture.invalid'],cwd:root,cols:80,rows:24,
    env:{...process.env,HOME:root,HEY_BOSS_HEALTH_DIR:join(root,'health'),PATH:join(root,'bin')+':'+process.env.PATH,TERM:'xterm-256color'}});
  const wait=text=>session.waitFor(text,{scope:'screen',timeout:5000});
  const resize=async(width,height)=>{
    await session.resize(width,height);
    // SIGWINCH and ratatui's redraw are asynchronous; old text may already match.
    await delay(300);
    await session.waitForQuiet(150);
  };
  const capture=async name=>{
    await session.waitForQuiet(100);
    const screen=await session.screen();
    await renderTerminalPng(screen.rawLines.join('\n'),{output:join(output,name+'.png')});
    return screen.text;
  };
  await wait('Automatic: false');
  await session.type('3');await wait('very-long-project-name');
  await capture('worktree-list');
  await session.press('Enter');await wait('Selected entry');
  for(const [width,height] of [[120,24],[80,24],[48,20]]) {
    await resize(width,height);await wait('fixture-session');await wait('receipts');
    const screen=await capture(`details-${width}x${height}`);
    assert(screen.includes('Esc Back'));
    checks.push(`Full ownership and preservation details at ${width}x${height}`);
  }
  await resize(48,14);await wait('Selected entry');
  for(let i=0;i<4;i++)await session.press('ArrowDown');
  await wait('receipts');await capture('details-scrolled');
  checks.push('Long details remain reachable in a short terminal');
  await session.press('Escape');await wait('Enter Details');
  await resize(80,24);
  await session.type('x');await delay(300);await capture('remove-confirmation');
  await wait('Safety checks still apply');
  const confirmation=await capture('remove-confirmation');
  assert(confirmation.includes('any other key')&&confirmation.includes('cancels'));
  await session.type('n');await wait('Enter Details');
  checks.push('Details return to the list and removal can be cancelled');
  for (const message of ['Missing checkout; metadata preserved', 'Modified, untracked, or ignored files; preserved']) {
    await session.press('ArrowDown');await session.press('Enter');
    await wait(message);await session.type('x');
    await wait('Selected entry');
    await capture(message.startsWith('Missing')?'missing-metadata':'uncommitted-work');
    checks.push(message+' remains readable and details do not dispatch cleanup');
    await session.press('Escape');await wait('Enter Details');
  }
  await session.type('q');assert.equal(await session.waitForExit({timeout:5000}),0);
  const calls=await readFile(join(root,'calls'),'utf8');
  assert(!calls.includes('remove-worktree')&&!calls.includes("'clean'"),'Viewing details must never mutate maintenance state');
  checks.push('Normal exit with no maintenance mutations');
  assert.equal(checks.length,8);
  console.log(JSON.stringify({normalCompletion:true,completed:8,expected:8,checks}));
} finally {
  if(pilot)await pilot.close();
  await rm(root,{recursive:true,force:true});
}
