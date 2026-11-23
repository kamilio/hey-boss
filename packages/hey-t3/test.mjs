import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, existsSync, rmSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import assert from 'node:assert/strict';
import { target, desiredCommand, recapPage, serverCLI } from './hey-t3.mjs';

test('authentication follows the live server across desktop nightly updates', async (t) => {
  const home = mkdtempSync(join(process.cwd(), '.hey-t3-test-'));
  const cli = (path, version) => {
    mkdirSync(path, { recursive: true });
    writeFileSync(join(path, 't3'), `#!/bin/sh\necho 't3 v${version}'\n`, { mode: 0o755 });
  };
  try {
    const current = '0.1.12-nightly.20261123.5138', running = '0.1.11-nightly.20261119.4928';
    cli(join(home, 'bin'), current);
    const name = `t3-${running}-${process.platform}-${process.arch}`;
    cli(join(home, name), running);
    const archive = join(home, 'runtime.tar.gz');
    assert.equal(spawnSync('tar', ['-czf', archive, '-C', home, name]).status, 0);
    const bytes = readFileSync(archive), checksum = createHash('sha256').update(bytes).digest('hex');
    const urls = [];
    t.mock.method(globalThis, 'fetch', async url => {
      urls.push(url);
      return new Response(url.endsWith('/SHA256SUMS') ? `${checksum}  ${name}.tar.gz\n` : bytes);
    });
    assert.equal(await serverCLI(home, current), join(home, 'bin/t3'));
    assert.equal(urls.length, 0);
    const cached = await serverCLI(home, running);
    assert.equal(cached, join(home, 'caches/hey-t3', running, 't3'));
    assert.deepEqual(urls, ['SHA256SUMS', `${name}.tar.gz`].map(file => `https://github.com/pingdotgg/t3code/releases/download/v${running}/${file}`));
    assert.equal(await serverCLI(home, running), cached);
    assert.equal(urls.length, 2);
    // Once the server restarts into the new nightly, use its installed CLI.
    assert.equal(await serverCLI(home, current), join(home, 'bin/t3'));
    await assert.rejects(serverCLI(home, '../invalid'), /Invalid T3 server version/);
    assert.equal(urls.length, 2);
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test('a damaged CLI download is rejected and its staging files are removed', async (t) => {
  const home = mkdtempSync(join(process.cwd(), '.hey-t3-test-'));
  const version = '1.2.3', name = `t3-${version}-${process.platform}-${process.arch}.tar.gz`;
  t.mock.method(globalThis, 'fetch', async url => new Response(
    url.endsWith('/SHA256SUMS') ? `${'0'.repeat(64)}  ${name}\n` : 'damaged archive',
  ));
  try {
    await assert.rejects(serverCLI(home, version), /checksum mismatch/);
    assert.equal(existsSync(join(home, 'caches/hey-t3', version)), false);
    assert.deepEqual(readdirSync(join(home, 'caches/hey-t3')), []);
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

const env = '00000000-0000-0000-0000-000000000001',
  id = '00000000-0000-0000-0000-000000000002';
test('Desktop route and shared link resolve to the same exact thread', () => {
  for (const link of [`t3code://threads/${env}/${id}`, `t3code://app/#/${env}/${id}`])
    assert.deepEqual(target(link), { environment: env, id });
  for (const link of [id, `https://threads/${env}/${id}`, `t3code://threads/${env}/${id}/extra`])
    assert.throws(() => target(link));
});
test('commands express desired state without toggling twice', () => {
  const thread = { id, title: 'Before', pinnedAt: null };
  assert.equal(desiredCommand('pin', thread)?.type, 'thread.pin');
  assert.equal(desiredCommand('pin', { ...thread, pinnedAt: 'now' }), null);
  assert.equal(desiredCommand('unpin', thread), null);
  assert.equal(desiredCommand('rename', thread, 'Before'), null);
  assert.equal(desiredCommand('rename', thread, 'After').title, 'After');
  assert.equal(desiredCommand('settle', { ...thread, settledOverride: 'settled' }), null);
});
test('recap pages stop at the last human, excluding tool output and internal messages', () => {
  const row = (type, text, createdBy) => ({ item: { type, text, createdBy } });
  const replies = [];
  assert.equal(
    recapPage([row('user_message', 'internal', 'agent'), row('assistant_message', 'Done.')], replies),
    null,
  );
  assert.deepEqual(
    recapPage(
      [
        row('assistant_message', 'old'),
        row('user_message', 'Question', 'user'),
        row('reasoning', 'hidden'),
        row('assistant_message', 'Checking.'),
      ],
      replies,
    ),
    { lastMessage: 'Question', replies: 'Checking.\n\nDone.' },
  );
  assert.throws(() => recapPage([row('user_message', 'Question', 'user')], []), /No replies/);
});

test('an offline companion does not fall back to an old SSH registration', () => {
  const home = mkdtempSync(join(process.cwd(), '.hey-t3-test-'));
  try {
    const old = join(home, '.config/hey-t3/hosts');
    mkdirSync(old, { recursive: true });
    writeFileSync(join(old, env), '-invalid-old-host');
    const r = spawnSync(
      process.execPath,
      [fileURLToPath(new URL('./hey-t3.mjs', import.meta.url)), 'show', `t3code://threads/${env}/${id}`],
      { env: { ...process.env, HOME: home }, encoding: 'utf8' },
    );
    assert.equal(r.status, 1);
    assert.match(r.stderr, /not connected in Hey Boss/);
    assert.equal(r.stdout, '');
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test('send requests active steering; queue never steers; both preserve the exact message', () => {
  const t = { id },
    text = 'First line\n\nSecond line $()';
  const send = desiredCommand('send', t, text),
    queue = desiredCommand('queue', t, text);
  assert.equal(send.type, 'message.dispatch');
  assert.equal(send.deliveryIntent, 'steer');
  assert.equal(send.dispatchMode.type, 'start_immediately');
  assert.equal(queue.deliveryIntent, undefined);
  assert.equal(queue.dispatchMode.type, 'queue_after_active');
  assert.equal(send.text, text);
  assert.equal(queue.text, text);
  assert.notEqual(send.messageId, queue.messageId);
});

test('start chooses a model on the target host without changing accounts silently', async () => {
  const { selectModel } = await import('./hey-t3.mjs');
  const model = {
    slug: 'gpt-6.1-sol',
    capabilities: {
      optionDescriptors: [{ id: 'reasoningEffort', options: [{ id: 'medium' }, { id: 'high' }] }],
    },
  };
  const providers = ['codex', 'work'].map((instanceId) => ({
    instanceId,
    enabled: true,
    models: [model, { slug: 'other' }],
  }));
  const config = { providers, settings: {} };
  const standard = {
    instanceId: 'codex',
    model: model.slug,
    options: [{ id: 'reasoningEffort', value: 'medium' }],
  };
  assert.deepEqual(selectModel(config, {}, {}), standard);
  const project = { defaultModelSelection: { ...standard, instanceId: 'work' } };
  assert.equal(selectModel(config, project, { model: 'other' }).instanceId, 'work');
  assert.equal(selectModel(config, project, { effort: 'high' }).options[0].value, 'high');
  assert.throws(() => selectModel(config, {}, { model: 'missing' }), /model/i);
  assert.throws(() => selectModel(config, {}, { model: 'other', effort: 'high' }), /effort/i);
});

test('server-generated thread IDs round trip without accepting path or query fragments', () => {
  for (const thread of [`mcp:${id}`, `thread:project:${env}:${id}`]) {
    assert.equal(target(`t3code://threads/${env}/${thread}`).id, thread);
  }
  for (const thread of ['..', '%2Fbad', '?x', '#fragment', '']) {
    assert.throws(() => target(`t3code://threads/${env}/${thread}`));
  }
});

test('a delegation returns only its own completed reply', async () => {
  const {delegateResult} = await import('./hey-t3.mjs');
  const run = {id:'r', userMessageId:'m', status:'running'};
  const projection = {runs:[run], visibleTurnItems:[
    {item:{type:'assistant_message',runId:'r',text:'Progress'}},
    {item:{type:'assistant_message',runId:'r',text:'Final answer'}},
    {item:{type:'assistant_message',runId:'other',text:'Unrelated'}},
  ]};
  assert.equal(delegateResult(projection,'m'),null);
  assert.equal(delegateResult(projection,'missing'),null);
  run.status='failed';
  assert.throws(()=>delegateResult(projection,'m'),/failed/);
  run.status='completed';
  assert.equal(delegateResult(projection,'m'),'Final answer');
});
