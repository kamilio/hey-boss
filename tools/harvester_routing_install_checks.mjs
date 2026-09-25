// Verify installed SSH dispatch without network access or real maintenance state.
import assert from 'node:assert/strict';
import {mkdtempSync, realpathSync, mkdirSync, writeFileSync, renameSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {basename, join, resolve} from 'node:path';
import {spawnSync} from 'node:child_process';

const binary=resolve(process.argv[2]);
const root=realpathSync(mkdtempSync(join(tmpdir(),'hb-harvester-routing-')));
const checks=[];
const script=(path,body)=>writeFileSync(path,'#!/bin/sh\nset -eu\n'+body,{mode:0o700});
try {
  for(const path of ['bin','.local/bin','.cargo/bin','health'])mkdirSync(join(root,path),{recursive:true});
  // Execute only the generated remote shell command, inside the fixture HOME.
  script(join(root,'bin/ssh'),'for arg do remote_script=$arg; done\nexec /bin/sh -c "$remote_script"\n');
  const modern=join(root,'.local/bin/hey-harvester');
  const alternate=join(root,'.cargo/bin/hey-harvester');
  script(modern,'printf \'{"worker":"harvester","args":"%s"}\\n\' "$*"\n');
  script(join(root,'.local/bin/hey-boss-health'),'printf \'{"worker":"legacy","args":"%s"}\\n\' "$*"\n');
  const invoke=()=>{
    const args=[...(basename(binary)==='hey-harvester'?[]:['health']),'--host','routing-fixture.invalid','status','--json'];
    const result=spawnSync(binary,args,{cwd:root,env:{...process.env,HOME:root,PATH:join(root,'bin')+':'+process.env.PATH,HEY_BOSS_HEALTH_DIR:join(root,'health')},encoding:'utf8',timeout:45000});
    assert(!result.error && result.signal===null && result.status!==null,`Incomplete routing check: ${result.error||result.signal}`);
    return result;
  };
  const run=()=>{
    const result=invoke();
    assert.equal(result.status,0,result.stderr);
    return JSON.parse(result.stdout);
  };
  assert.deepEqual(run(),{worker:'harvester',args:'status --json'});
  checks.push('Local harvester takes precedence over stale legacy helper');
  renameSync(modern,alternate);
  assert.deepEqual(run(),{worker:'harvester',args:'status --json'});
  checks.push('Cargo-installed harvester also takes precedence over legacy helper');
  // A failing modern worker must not silently fall back to an obsolete helper.
  script(alternate,'exit 23\n');
  const failure=invoke();
  assert.notEqual(failure.status,0,'Failed harvester must remain a failure');
  assert(!failure.stdout.includes('legacy'),'Failed harvester must not dispatch the stale helper');
  checks.push('Modern worker failure never falls through to the stale helper');
  assert.equal(checks.length,3);
  console.log(JSON.stringify({normalCompletion:true,completed:3,expected:3,checks}));
} finally {
  rmSync(root,{recursive:true,force:true});
}
