import {test} from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,writeFileSync,readFileSync,rmSync,readdirSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {join,resolve} from 'node:path';

test('Wire adapter creates, updates, rejects remote edits, and cleans its workspaces',()=>{
  const root=mkdtempSync(join(process.cwd(),'.notion-adapter-test-'));
  try {
    const wire=join(root,'wire');
    writeFileSync(wire,`#!/usr/bin/env node
const fs=require('fs'),path=require('path');
const root=path.dirname(process.argv[1]),remote=path.join(root,'remote.md');
const [op,value]=process.argv.slice(2);
const url='https://www.notion.so/0123456789abcdef0123456789abcdef';
if(op==='init') { console.log('{}'); }
else if(op==='attach') { const file=path.join(process.cwd(),'attached.md'),markdown=fs.readFileSync(remote,'utf8'); fs.writeFileSync(file,markdown); console.log(JSON.stringify({path:file,markdown,summary:{remote:url}})); }
else if(op==='sync') { const markdown=fs.readFileSync(value,'utf8');fs.writeFileSync(remote,markdown);console.log(JSON.stringify({markdown,summary:{remote:url}})); }
else process.exit(1);
`,{mode:0o700});
    const source=join(root,'document.md');
    writeFileSync(source,'# Plan\n\nFirst revision');
    const run=(...args)=>spawnSync(process.execPath,[resolve('tools/notion-publisher.mjs'),...args],{cwd:root,env:{...process.env,PATH:root+':'+process.env.PATH},encoding:'utf8'});
    let result=run('create',source,'Plan');assert.equal(result.status,0,result.stderr);
    const publication=JSON.parse(result.stdout);assert.match(publication.url,/notion.so/);
    assert.equal(readFileSync(join(root,'remote.md'),'utf8'),'# Plan\n\n\nFirst revision');
    const read=run('read',publication.id);assert.equal(read.status,0,read.stderr);assert.equal(JSON.parse(read.stdout).version,publication.version);
    writeFileSync(source,'Second revision');
    result=run('update',source,'Plan',publication.id,publication.version);assert.equal(result.status,0,result.stderr);
    const updated=JSON.parse(result.stdout);assert.equal(updated.url,publication.url);assert.notEqual(updated.version,publication.version);
    writeFileSync(join(root,'remote.md'),'Remote edit');
    result=run('update',source,'Plan',updated.id,updated.version);assert.notEqual(result.status,0);assert.match(result.stderr,/changed since/);
    assert.equal(readFileSync(join(root,'remote.md'),'utf8'),'Remote edit');
    assert.equal(readdirSync(root).some(name=>name.startsWith('.notion-')),false);
  } finally { rmSync(root,{recursive:true,force:true}); }
});
