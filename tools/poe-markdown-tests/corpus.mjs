import fs from 'node:fs';
import {canonical} from './semantics.mjs';
import {createHash} from 'node:crypto';
const base=new URL('./',import.meta.url);
const read=p=>JSON.parse(fs.readFileSync(new URL(p,base)));
const suite=read('report.json');
if(suite.numFailedTests || suite.numPendingTests || suite.numPassedTests!==suite.numTotalTests) throw Error('Complete upstream suite must pass before exporting');
const cases=fs.readdirSync(new URL('captured/',base)).map(f=>read('captured/'+f)).sort((a,b)=>a.name.localeCompare(b.name));
if(cases.length!==suite.numTotalTests) throw Error('Missing captured test cases');
const sources=new Map(), codes=new Map(), asts=[];
for(const test of cases) for(const call of test.calls) {
  if(call.source!==undefined) {
    const id=createHash('sha256').update(call.source).digest('hex').slice(0,16);
    if(!sources.has(id)) sources.set(id,{id,source:call.source,expected:canonical(call.ast),tests:[]});
    sources.get(id).tests.push(test.name);
  } else if(call.operation==='highlightCodeBlock') {
    const {value,lang=''}=call.args[0];
    const id=createHash('sha256').update(lang+'\0'+value).digest('hex').slice(0,16);
    codes.set(id,{id,source:value,lang,tokens:call.result,tests:[test.name]});
  } else if(call.ast) asts.push({test:test.name,ast:call.ast});
}
const corpus={revision:read('upstream-manifest.json').revision,testCount:suite.numTotalTests,tests:cases.map(c=>({name:c.name,operations:[...new Set(c.calls.map(c=>c.operation))]})),documents:[...sources.values()].sort((a,b)=>a.id.localeCompare(b.id)),code:[...codes.values()].sort((a,b)=>a.id.localeCompare(b.id)),renderNodes:asts};
fs.writeFileSync(new URL('../../tests/fixtures/poe-markdown-suite.json',base),JSON.stringify(corpus,null,2)+'\n');
console.log(`${corpus.testCount} tests; ${corpus.documents.length} Markdown inputs; ${corpus.code.length} code inputs; ${asts.length} direct renderer inputs`);
