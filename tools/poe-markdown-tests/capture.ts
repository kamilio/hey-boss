import fs from 'node:fs';
import {createHash} from 'node:crypto';
import {afterEach, beforeEach, expect} from 'vitest';
import {parse} from './upstream/packages/toolcraft-design/src/terminal-markdown/parser.js';
let calls: unknown[] = [];
function plain(value: unknown) {
  const parents: object[] = [];
  return JSON.parse(JSON.stringify(value,function (_,v) {
    if (v && typeof v === 'object') {
      while (parents.length && parents.at(-1) !== this) parents.pop();
      if (parents.includes(v)) return '[Circular]';
      parents.push(v);
    }
    return v;
  }) ?? 'null');
}
(globalThis as any).__poeCapture = (operation: string, args: any[], result: any) => {
  if (!process.env.POE_MARKDOWN_CAPTURE) return;
  let source = typeof args[0] === 'string' ? args[0] : undefined;
  if(operation === 'getMarkdownDemo') source=result;
  const ast = source !== undefined ? parse(source).ast : args[0]?.type ? args[0] : undefined;
  calls.push(plain({operation,args,result,source,ast}));
};
beforeEach(()=>{calls=[];});
afterEach(()=>{
  if (!process.env.POE_MARKDOWN_CAPTURE) return;
  const name=expect.getState().currentTestName!;
  fs.mkdirSync('captured',{recursive:true});
  fs.writeFileSync('captured/'+createHash('sha256').update(name).digest('hex')+'.json',JSON.stringify({name,calls},null,2)+'\n');
});
