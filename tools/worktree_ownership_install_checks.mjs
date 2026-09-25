// Exercise the installed CLI against an owned disposable repository, never a
// real project, validator, health configuration or worktree.
import assert from 'node:assert/strict';
import {mkdtempSync, realpathSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {basename, join, resolve} from 'node:path';
import {spawnSync} from 'node:child_process';
const binary=resolve(process.argv[2]);
// The extracted worker exposes maintenance commands directly; legacy binaries
// retain the `health` command group during a rolling installation.
const command=basename(binary)==='hey-harvester'?[]:['health'];
const root=realpathSync(mkdtempSync(join(tmpdir(),'hb-ownership-qa-')));
const main=join(root,'main'), work=join(root,'owned');
const env={...process.env,HEY_BOSS_HEALTH_DIR:join(root,'health')};
const run=(bin,args,cwd=main)=>{
  const r=spawnSync(bin,args,{cwd,env,encoding:'utf8',timeout:120000});
  assert(!r.error && r.signal===null && r.status!==null,`Incomplete command ${bin}: ${r.error||r.signal}`);
  assert.equal(r.status,0,`${bin}: ${r.stderr.slice(0,2000)}`);
  return r.stdout+r.stderr;
};
const git=args=>run('git',['-c','core.hooksPath=/dev/null',...args]);
const checks=[];
try {
  mkdirSync(main);git(['init','-b','main']);git(['config','user.email','fixture@example.invalid']);git(['config','user.name','Fixture']);
  writeFileSync(join(main,'file'),'committed');git(['add','file']);git(['commit','-m','fixture']);
  const head=git(['rev-parse','HEAD']).trim();
  const reason='issue 147; owner installation-fixture; queued validation';
  git(['worktree','add','--lock','--reason',reason,'-b','owned',work]);
  const admin=run('git',['rev-parse','--absolute-git-dir'],work).trim();
  const receipt=join(root,'receipt');writeFileSync(receipt,'pending');
  for(const state of ['queued','interrupted']) {
    const reply=JSON.parse(run(binary,[...command,'remove-worktree',work,'--json']));
    assert(reply.errors.some(error=>error.includes(reason)),'Installed CLI must expose ownership reason');
    assert.equal(reply.phase,'Worktree preserved');
    assert(existsSync(work)&&existsSync(admin));
    assert(git(['worktree','list','--porcelain']).includes(reason));
    checks.push(`${state}: removal refused with ownership visible`);
  }
  writeFileSync(join(work,'file'),'staged recovery work');run('git',['add','file'],work);
  const index=readFileSync(join(admin,'index'));
  const refusal=JSON.parse(run(binary,[...command,'remove-worktree',work,'--json']));
  assert(refusal.errors.some(error=>error.includes(reason)));
  assert(readFileSync(join(admin,'index')).equals(index));
  assert.equal(run('git',['show',':file'],work).trim(),'staged recovery work');
  assert.equal(run('git',['rev-parse','HEAD'],work).trim(),head);
  assert.equal(readFileSync(receipt,'utf8'),'pending');
  checks.push('Recovery preserves index, staged content, HEAD and receipt');
  run('git',['-c','core.hooksPath=/dev/null','commit','-m','completed fixture'],work);
  const final=run('git',['rev-parse','HEAD'],work).trim();writeFileSync(receipt,'complete');
  git(['worktree','unlock',work]);git(['worktree','remove','--',work]);
  assert(!existsSync(work)&&!existsSync(admin));
  assert.equal(git(['rev-parse','refs/heads/owned']).trim(),final);
  assert.equal(readFileSync(receipt,'utf8'),'complete');
  checks.push('Explicit completed-owner cleanup retains branch and receipt');
  assert.equal(checks.length,4);
  console.log(JSON.stringify({normalCompletion:true,completed:4,expected:4,checks}));
} finally {
  // root is exclusively this script's mkdtemp fixture, not a user worktree.
  rmSync(root,{recursive:true,force:true});
}
