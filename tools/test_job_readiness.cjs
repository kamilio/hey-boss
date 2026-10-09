const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('src/issues/web/app.js', 'utf8');
const start = source.indexOf('function renderReadiness(');
const ctx = vm.createContext({esc: value => String(value).replaceAll('<','&lt;')});
vm.runInContext(source.slice(start, source.indexOf('\nfunction ', start+1)),ctx);
for (const state of ['pending','running','succeeded','failed','cancelled']) {
  const html=ctx.renderReadiness({issue:{job_run_id:'execution',state:'open',status:{comment:'Job '+state}},allocation:{role:'agent'}});
  assert.ok(html.includes('>'+state[0].toUpperCase()+state.slice(1)+'</strong>'));
  assert.ok(html.includes('independently of worker slots'));
  assert.ok(!html.includes('Ready for agents'));
  assert.ok(!html.includes('Move to draft'));
}
assert.equal(ctx.renderReadiness({issue:{job_run_id:'execution',deleted_at:1}}),'');
console.log('Scheduled job readiness checks passed');
