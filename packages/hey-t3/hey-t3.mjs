#!/usr/bin/env node
import { execFile, execFileSync, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync, readdirSync, readFileSync, realpathSync } from 'node:fs';
import { promisify } from 'node:util';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';

const uuid = /^[a-f0-9]{8}-(?:[a-f0-9]{4}-){3}[a-f0-9]{12}$/i;
const usage = `hey-t3 list [ENVIRONMENT_ID]
hey-t3 show|pull|catch-up|pin|unpin|settle THREAD_LINK
hey-t3 rename THREAD_LINK TITLE
hey-t3 send|queue THREAD_LINK MESSAGE (use - to read stdin)

Reads and changes print JSON. Use the link field from hey-t3 list.
Remote threads use machines already connected in Hey Boss.`;
const home = join(homedir(), '.t3');
const idFile = join(home, 'userdata/environment-id');
const localID = () => readFileSync(idFile, 'utf8').trim().toLowerCase();

export function target(link) {
  const parts = link.replace(/^t3code:\/\/(?:threads\/|app\/#\/)/, '').split('/');
  if (parts.length !== 2 || parts.join('/') === link || !parts.every((x) => uuid.test(x)))
    throw Error('Expected a T3 thread link with an environment ID and thread ID.');
  return { environment: parts[0].toLowerCase(), id: parts[1].toLowerCase() };
}

export function recapPage(items, replies) {
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i].item;
    if (item?.type === 'user_message' && (item.createdBy ?? 'user') === 'user') {
      if (!replies.length) throw Error('No replies since your last message.');
      return { lastMessage: item.text ?? '', replies: replies.reverse().join('\n\n') };
    }
    if (item?.type === 'assistant_message' && item.text?.trim()) replies.push(item.text);
  }
  return null;
}

export function desiredCommand(op, t, text) {
  const c = { commandId: randomUUID(), threadId: t.id };
  switch (op) {
    case 'send':
    case 'queue':
      return {
        ...c,
        type: 'message.dispatch',
        messageId: randomUUID(),
        text,
        attachments: [],
        createdBy: 'user',
        creationSource: 'web',
        ...(op === 'send' ? { deliveryIntent: 'steer' } : {}),
        dispatchMode: { type: op === 'send' ? 'start_immediately' : 'queue_after_active' },
      };
    case 'pin':
      return t.pinnedAt ? null : { ...c, type: 'thread.pin' };
    case 'unpin':
      return t.pinnedAt ? { ...c, type: 'thread.unpin' } : null;
    case 'settle':
      return !t.pinnedAt && t.settledOverride === 'settled' ? null : { ...c, type: 'thread.settle' };
    case 'rename':
      return t.title === text ? null : { ...c, type: 'thread.metadata.update', title: text };
    default:
      throw Error('Invalid thread action.');
  }
}

async function onHost({ op, environment, id, text, guarded }, lines) {
  const actual = localID();
  if (actual !== environment) throw Error('The connected machine no longer serves this T3 environment.');
  const { origin, serverVersion } = JSON.parse(
    readFileSync(join(home, 'userdata/server-runtime.json'), 'utf8'),
  );
  const base = new URL(origin);
  if (base.protocol !== 'http:' || !['127.0.0.1', '[::1]', 'localhost'].includes(base.hostname))
    throw Error('Expected a local T3 server.');
  const get = async (path, headers = {}) => {
    const r = await fetch(new URL(path, base), {
      headers,
      redirect: 'error',
      signal: AbortSignal.timeout(30000),
    });
    if (!r.ok) {
      const detail = await r.json().catch(() => ({}));
      if (detail.reason === 'thread_transcript_too_large')
        throw Error('This transcript exceeds T3’s 50 MiB export limit. Use show for the conversation text.');
      throw Error(`T3 returned HTTP ${r.status}.`);
    }
    return r.json();
  };
  if ((await get('/.well-known/t3/environment')).environmentId !== environment)
    throw Error('The running T3 server does not match the thread link.');
  let cli = join(home, 'bin/t3');
  if (!existsSync(cli)) {
    if (!/^[0-9][a-zA-Z0-9.+-]*$/.test(serverVersion)) throw Error('Invalid T3 version.');
    cli = join(home, 'runtime/versions', serverVersion, 't3');
  }
  const write = ['pin', 'unpin', 'settle', 'rename', 'send', 'queue'].includes(op);
  const args = ['auth', 'session', 'issue', '--scope', 'orchestration:read'];
  if (write) args.push('--scope', 'orchestration:operate');
  const token = execFileSync(cli, [...args, '--ttl', '1m', '--label', 'hey-t3', '--token-only'], {
    encoding: 'utf8',
    timeout: 12000,
    stdio: ['ignore', 'pipe', 'ignore'],
  }).trim();
  const headers = { authorization: 'Bearer ' + token, 'x-t3-orchestration-protocol': '2' };
  if (op === 'list') {
    const { threads } = await get('/api/orchestration/shell?omitIdleSubagents=1&omitOlderSettled=1', headers);
    return threads
      .filter((t) => t.lineage?.relationshipToParent !== 'subagent')
      .map((t) => ({ ...t, link: `t3code://threads/${environment}/${t.id}` }));
  }
  const path = '/api/orchestration/threads/' + id;
  const bounded = async (compact) => {
    const page = await get(path + '/bounded?deferWork=1' + (compact ? '&compactTurnItems=1' : ''), headers);
    if (page.projection?.thread?.id !== id) throw Error('T3 returned a different thread.');
    return page;
  };
  if (op === 'pull') {
    const transcript = await get(path + '/transcript', headers);
    if (transcript.threadId !== id) throw Error('T3 returned a different thread.');
    return transcript;
  }
  const page = op === 'show' ? await get(path, headers) : await bounded(write),
    thread = page.projection.thread;
  if (thread.id !== id) throw Error('T3 returned a different thread.');
  if (op === 'show')
    return {
      ...thread,
      messages: page.projection.visibleTurnItems
        .map((row) => row.item)
        .filter(
          (item) =>
            item?.type === 'assistant_message' ||
            (item?.type === 'user_message' && (item.createdBy ?? 'user') === 'user'),
        )
        .map((item) => ({ role: item.type === 'assistant_message' ? 'assistant' : 'user', text: item.text })),
    };
  if (op === 'catch-up') {
    let items = page.projection.visibleTurnItems,
      more = page.hasMoreHistory,
      cursor = page.historyCursor;
    const replies = [],
      seen = new Set();
    for (;;) {
      const recap = recapPage(items, replies);
      if (recap) return recap;
      if (!more) throw Error('No message from you in this thread yet.');
      if (!cursor || seen.has(cursor)) throw Error('T3 could not read earlier messages.');
      seen.add(cursor);
      const prior = await get(
        path + '/history?' + new URLSearchParams({ cursor, view: 'conversation' }),
        headers,
      );
      items = prior.items;
      more = prior.hasMoreHistory;
      cursor = prior.nextCursor;
    }
  }
  const command = desiredCommand(op, thread, text);
  if (!command) return thread;
  const ws = new WebSocket(new URL('/ws?orchestrationProtocol=2', base), { headers });
  try {
    await new Promise((resolve, reject) => {
      const fail = () => reject(Error('T3 did not confirm the command.'));
      ws.onerror = ws.onclose = fail;
      ws.onopen = async () => {
        try {
          if (guarded) {
            console.log('READY');
            if ((await lines.next()).value !== 'GO') throw Error('Thread action cancelled.');
          }
          ws.send(
            JSON.stringify({
              _tag: 'Request',
              id: '1',
              tag: 'orchestration.dispatchCommand',
              payload: command,
              headers: [],
            }),
          );
        } catch (e) {
          reject(e);
        }
      };
      ws.onmessage = async ({ data }) => {
        try {
          const m = JSON.parse(typeof data === 'string' ? data : await data.text());
          if (m._tag === 'Ping') ws.send('{"_tag":"Pong"}');
          if (m.requestId === '1') m.exit?._tag === 'Success' ? resolve() : fail();
        } catch (e) {
          reject(e);
        }
      };
      AbortSignal.timeout(10000).addEventListener('abort', fail, { once: true });
    });
  } finally {
    ws.close();
  }

  if (command.type === 'message.dispatch') {
    const receipt = { threadId: id, messageId: command.messageId };
    // Dispatch was acknowledged. A missing readback must not invite a duplicate send.
    const projection = await bounded(false)
      .then((page) => page.projection)
      .catch(() => null);
    if (!projection) return { ...receipt, delivery: 'accepted' };
    const item = projection.visibleTurnItems.find(({ item }) => item.messageId === command.messageId)?.item;
    const run = projection.runs.find((run) => run.userMessageId === command.messageId);
    const delivery =
      { turn_start: 'started', steer: 'steered', promoted_queued_to_steer: 'steered', queued_turn: 'queued' }[
        item?.inputIntent
      ] ?? (run?.status === 'queued' ? 'queued' : 'accepted');
    return { ...receipt, delivery };
  }
  const after = (await bounded(true)).projection.thread;

  if (desiredCommand(op, after, text)) throw Error('T3 did not apply the requested change.');
  return after;
}

// OpenSSH normally falls back to a fresh login if a master disappears.
// Use only the existing Hey Boss socket, even if it closes after discovery.
function companionArgs(socket) {
  return [
    '-F',
    '/dev/null',
    '-T',
    '-S',
    socket,
    '-o',
    'BatchMode=yes',
    '-o',
    'ProxyCommand=/usr/bin/false',
    'localhost',
  ];
}

async function connection(environment) {
  const dir = join(homedir(), '.local/share/hey-boss');
  const sockets = existsSync(dir)
    ? readdirSync(dir, { withFileTypes: true }).filter(
        (entry) => entry.isSocket() && /^ssh-[a-f0-9]{16}\.sock$/.test(entry.name),
      )
    : [];
  try {
    return await Promise.any(
      sockets.map(async ({ name }) => {
        const path = join(dir, name);
        const { stdout } = await promisify(execFile)(
          '/usr/bin/ssh',
          [...companionArgs(path), 'cat "$HOME/.t3/userdata/environment-id"'],
          { encoding: 'utf8', timeout: 3000, maxBuffer: 8192 },
        );
        if (stdout.trim().split(/\r?\n/).at(-1)?.toLowerCase() !== environment) throw Error();
        return path;
      }),
    );
  } catch {
    throw Error('This thread’s machine is not connected in Hey Boss, or T3 is unavailable on it.');
  }
}

async function main(args) {
  if (!args.length || args[0] === '--help') {
    console.log(usage);
    return;
  }
  const guarded = args[0] === '--check-focus';
  if (guarded) args.shift();
  const [op, arg, value] = args;
  if (!(
    (op === 'list' && args.length <= 2) ||
    (['send', 'queue', 'rename'].includes(op) && args.length === 3) ||
    (['show', 'pull', 'catch-up', 'pin', 'unpin', 'settle'].includes(op) && args.length === 2)
  ))
    throw Error(usage);
  const { environment, id } = op === 'list' ? { environment: (arg ?? localID()).toLowerCase() } : target(arg);
  if (!uuid.test(environment)) throw Error('Invalid environment ID.');
  const text = ['send', 'queue'].includes(op) && value === '-' ? readFileSync(0, 'utf8') : value;
  if (text !== undefined && !text.trim()) throw Error('Message or title cannot be empty.');
  const input = { op, environment, id, text, guarded };

  const stdinLines = () => createInterface({ input: process.stdin })[Symbol.asyncIterator]();
  if (existsSync(idFile) && environment === localID()) {
    console.log(JSON.stringify(await onHost(input, guarded ? stdinLines() : null)));
    return;
  }
  const socket = await connection(environment);
  // Run the same source on the owning host. Only fixed program text enters
  // the remote shell; thread data and the focus acknowledgement travel on stdin.
  const source = readFileSync(fileURLToPath(import.meta.url), 'utf8').replaceAll("'", "'\\''");
  const entry = `PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH" exec node --input-type=module -e '${source}' -- --worker`;
  const child = spawn('/usr/bin/ssh', [...companionArgs(socket), entry], {
    stdio: ['pipe', 'pipe', 'inherit'],
    timeout: 90000,
  });
  const done = new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('close', resolve);
  });
  child.stdin.on('error', () => {});
  child.stdin.write(JSON.stringify(input) + '\n');
  if (!guarded) child.stdin.end();
  let result;
  for await (const line of createInterface({ input: child.stdout })) {
    if (line === 'READY' && guarded) {
      console.log('READY');
      child.stdin.end((await stdinLines().next()).value === 'GO' ? 'GO\n' : 'STOP\n');
    } else result = line;
  }
  const status = await done;
  if (status !== 0) {
    if (status === 255 || status === null)
      throw Error('The Hey Boss connection to this thread’s machine closed.');
    process.exitCode = status;
    return; // The worker already printed the cause on stderr.
  }
  console.log(JSON.stringify(JSON.parse(result)));
}

if (
  process.argv[1] === '--worker' ||
  (process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url))
) {
  try {
    if (process.argv[1] === '--worker') {
      const lines = createInterface({ input: process.stdin })[Symbol.asyncIterator]();
      console.log(JSON.stringify(await onHost(JSON.parse((await lines.next()).value), lines)));
    } else await main(process.argv.slice(2));
  } catch (e) {
    console.error(e.message);
    process.exitCode = 1;
  }
  process.stdin.unref?.();
}
