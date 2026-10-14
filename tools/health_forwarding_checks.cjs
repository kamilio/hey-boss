const {mkdtempSync,mkdirSync,writeFileSync,rmSync}=require('node:fs');
const {tmpdir}=require('node:os');
const {join,resolve}=require('node:path');
const {spawnSync}=require('node:child_process');
const assert=require('node:assert/strict');
const home=mkdtempSync(join(tmpdir(),'harvester-forward-'));
try {
 mkdirSync(join(home,'.local/bin'),{recursive:true});
 writeFileSync(join(home,'.local/bin/hey-harvester'),'#!/bin/sh\nprintf "%s\\n" "$@"\nexit 23\n',{mode:0o700});
 const r=spawnSync(resolve(process.argv[2]),['health','--host','fixture.invalid','status','--json'],{env:{...process.env,HOME:home},encoding:'utf8'});
 assert.equal(r.status,23,r.stderr);
 assert.equal(r.stdout,'--host\nfixture.invalid\nstatus\n--json\n');
 console.log('Health forwarding preserves arguments and exit status');
} finally {rmSync(home,{recursive:true,force:true});}
