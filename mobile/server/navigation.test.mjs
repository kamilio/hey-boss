import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';

test('mobile main navigation keeps core destinations without an Artifacts entry',()=>{
 const source=readFileSync(new URL('../src/main.jsx',import.meta.url),'utf8');
 const navigation=source.match(/<nav aria-label="Main navigation">([\s\S]*?)<\/nav>/)?.[1];
 assert.ok(navigation,'Main navigation is present');
 for(const label of ['Inbox','Activity','Issues','Agents'])assert.ok(navigation.includes(label),label+' remains available');
 assert.doesNotMatch(navigation,/artifacts/i);
});
