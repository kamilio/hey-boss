const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const {webcrypto, createHash, randomBytes} = require('node:crypto');
const vm = require('node:vm');

const source = readFileSync('src/issues/web/components.js', 'utf8');
function helpers(crypto) {
  const context = vm.createContext({crypto, TextEncoder, Uint8Array, Uint32Array, DataView});
  vm.runInContext(source + '\nthis.helpers = HeyBossUI;', context);
  return context.helpers;
}
(async () => {
  const ordinaryHTTP = helpers({getRandomValues: webcrypto.getRandomValues.bind(webcrypto)});
  const secure = helpers(webcrypto);
  for (const bytes of [Buffer.alloc(0), Buffer.from('abc'), Buffer.from('Zażółć 🌍'), ...[55,56,63,64,65,127,128,129,65536,1048576].map(size => randomBytes(size))]) {
    const expected = createHash('sha256').update(bytes).digest('hex');
    assert.equal(await ordinaryHTTP.sha256(bytes), expected);
    assert.equal(await secure.sha256(bytes), expected);
  }
  const ids = new Set();
  for (let n = 0; n < 1000; n++) {
    const id = ordinaryHTTP.requestId();
    assert.match(id, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    ids.add(id);
  }
  assert.equal(ids.size, 1000);
  assert.match(secure.requestId(), /^[0-9a-f-]{36}$/);
  console.log('COMPLETE: SHA-256 native/fallback match 13 vectors; 1000 unique RFC 9562 v4 IDs without secure-context APIs');
})().catch(error => {console.error(error);process.exitCode=1;});
