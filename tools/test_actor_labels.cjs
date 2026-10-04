const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const ctx = vm.createContext({document:{addEventListener(){}}, URLSearchParams});
vm.runInContext(fs.readFileSync('src/issues/web/components.js','utf8')+';globalThis.ui=HeyBossUI',ctx);
const name = ctx.ui.actorLabel;
assert.equal(name('codex:internal-session','gpt-6-astra'), 'Codex · gpt-6-astra');
assert.equal(name('claude:internal-session','claude-opus-4-6'), 'Claude · claude-opus-4-6');
assert.equal(name('worker:internal-run','gpt-6-sol'), 'Agent · gpt-6-sol');
for (const model of [null, '', {}, 'x'.repeat(257), '\u001bmodel']) {
  assert.equal(name('codex:internal-session',model), 'Codex · model unknown');
}
assert.equal(name('codex:internal-session',' custom/<model> '), 'Codex · custom/<model>');
assert.equal(name('human:boss','gpt-6-astra','Kamil'), 'Kamil');
assert.equal(name('human:alice'), 'alice');
assert.equal(name('watcher:github'), 'GitHub PR watcher');
assert.equal(name(null), 'Unassigned');
assert.equal(ctx.ui.rememberActors({actor_models:{'codex:internal-session':'gpt-6-sol'}}),true);
assert.equal(name('codex:internal-session'),'Codex · gpt-6-sol');
assert.equal(name('codex:internal-session','gpt-6-astra'),'Codex · gpt-6-astra','Recorded activity retains its original model');
assert.equal(ctx.ui.rememberActors({actor_models:{'codex:internal-session':'gpt-6-sol'}}),false);
console.log('Actor label checks passed');
