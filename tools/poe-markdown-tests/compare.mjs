import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {execFileSync} from 'node:child_process';
import {isDeepStrictEqual} from 'node:util';
import {canonical} from './semantics.mjs';
const root=new URL('../../',import.meta.url);
const corpus=JSON.parse(fs.readFileSync(new URL('tests/fixtures/poe-markdown-suite.json',root)));
const dir=fs.mkdtempSync(path.join(os.tmpdir(),'hb-poe-compare-'));
const mismatches=[];
try {
 for(const fixture of corpus.documents) {
  fs.writeFileSync(path.join(dir,'in.md'),fixture.source);
  execFileSync(process.env.HEY_BOSS_CLI_PATH??'/opt/homebrew/bin/hey-boss',['render-markdown',path.join(dir,'in.md'),path.join(dir,'out.json'),'--native']);
  const native=JSON.parse(fs.readFileSync(path.join(dir,'out.json')));
  const expected=fixture.expected, actual=canonical(native);

  if(!isDeepStrictEqual(expected,actual)) mismatches.push({id:fixture.id,tests:fixture.tests,source:fixture.source,expected,actual});
 }
 fs.writeFileSync(new URL('out/poe-mismatches.json',root),JSON.stringify(mismatches,null,2));

 console.log(`${corpus.documents.length-mismatches.length}/${corpus.documents.length} match; mismatches in out/poe-mismatches.json`);
} finally {fs.rmSync(dir,{recursive:true,force:true});}
