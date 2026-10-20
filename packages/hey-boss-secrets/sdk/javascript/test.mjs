import { test } from 'node:test';
import assert from 'node:assert/strict';
import { inspect } from 'node:util';
import { Client, Secret, SecretError } from './index.js';

test('secret wrappers redact inspection and serialization', () => {
  const secret = new Secret(Buffer.from('synthetic-only'));
  assert.equal(String(secret), '[redacted]');
  assert.equal(JSON.stringify({ secret }), '{"secret":"[redacted]"}');
  assert.equal(inspect(secret), 'Secret([redacted])');
  assert.equal(secret.expose().toString(), 'synthetic-only');
  secret.close(); assert(secret.expose().every(x => x === 0));
});
test('subprocess errors discard credential-bearing diagnostics', async () => {
  await assert.rejects(new Client('/does/not/exist').get('synthetic'), e => e instanceof SecretError && e.message === 'store_unavailable');
});
if (process.env.HEY_BOSS_SECRETS_TEST_BINARY) {
  test('installed CLI roundtrip, stale write and binary cookie blob', async () => {
    const client = new Client(process.env.HEY_BOSS_SECRETS_TEST_BINARY);
    const input = Buffer.from([0, 1, 255, 10, 13]);
    const record = await client.put('synthetic-js-sdk', input, { kind: 'cookies' });
    const result = await client.get('synthetic-js-sdk');
    assert.deepEqual(result.secret.expose(), input); assert.equal(result.record.revision, record.revision);
    await assert.rejects(client.put('synthetic-js-sdk', input, { expected: 'stale' }), e => e.message === 'stale_revision');
    await client.put('synthetic-js-sdk', Buffer.alloc(1024 * 1024, 171), { kind: 'blob', expected: record.revision });
    assert.equal((await client.get('synthetic-js-sdk')).secret.expose().length, 1024 * 1024);
  });
}
