import {test} from 'node:test';
import assert from 'node:assert/strict';
import {parseMarkdown} from '../src/markdown-worker.js';
function nodes(tree){return [tree,...(tree.children??[]).flatMap(nodes)];}
test('worker parser preserves GFM structure, entities and footnotes without raw HTML or DOM globals',async()=>{
 const tree=await parseMarkdown('# Heading &copy;\n\n**Bold** and ~~removed~~.\n\n- [x] Done\n- [ ] Waiting\n\n| Host | State |\n| --- | --- |\n| Mac | Ready |\n\n```swift\nlet x = true\n```\n\nReference[^one].\n\n[^one]: Footnote\n\n<script>bad()</script>');
 const all=nodes(tree),elements=all.filter(n=>n.type==='element');
 for(const name of ['h1','strong','del','ul','input','table','code','section'])assert.ok(elements.some(n=>n.tagName===name),name);
 assert.ok(all.some(n=>n.value==='Heading ©'));assert.equal(elements.filter(n=>n.tagName==='input').length,2);assert.ok(elements.find(n=>n.tagName==='input').properties.checked);
 assert.equal(elements.some(n=>n.tagName==='script'),false);assert.equal(all.some(n=>'position'in n),false);
 assert.ok(elements.some(n=>n.tagName==='a'&&n.properties.href?.startsWith('#')));
});
