#!/usr/bin/env node
import { execFile, execFileSync, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync, readdirSync, readFileSync, realpathSync } from 'node:fs';
import { parseArgs, promisify } from 'node:util';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';

const uuid = /^[a-f0-9]{8}-(?:[a-f0-9]{4}-){3}[a-f0-9]{12}$/i;
const usage = `hey-t3 projects [--env ID]
hey-t3 project PROJECT [--env ID]
hey-t3 list [ENVIRONMENT_ID] [--all] [--project PROJECT]
hey-t3 search QUERY [--project PROJECT] [--env ID]
hey-t3 start PROJECT [MESSAGE] [--title TITLE] [--worktree BASE] [--branch NAME]
             [--model MODEL] [--provider INSTANCE] [--effort LEVEL] [--env ID]
hey-t3 schedule THREAD_LINK SCHEDULE_JSON MESSAGE
hey-t3 promote THREAD_LINK QUEUED_RUN_ID
hey-t3 stop THREAD_LINK
hey-t3 show|pull|catch-up|pin|unpin|settle THREAD_LINK
hey-t3 rename THREAD_LINK TITLE
hey-t3 send|queue THREAD_LINK MESSAGE (use - to read stdin)

PROJECT is an ID, workspace root, or exact title. Omit MESSAGE to start idle.
Start uses the project or environment model, otherwise codex/gpt-6.1-sol;
reasoning defaults to medium when available. Use - for message stdin.
Schedule examples: {"type":"once","runAt":"2027-01-01T09:00:00Z"},
{"type":"interval","everyMs":3600000}, {"type":"fixed_time","timeOfDay":"09:30"}.
Reads and changes print JSON. Use the link field from hey-t3 list.
Remote threads use machines already connected in Hey Boss.`;
const home = join(homedir(), '.t3');
const idFile = join(home, 'userdata/environment-id');
const localID = () => readFileSync(idFile, 'utf8').trim().toLowerCase();

export function target(link) {
  const parts = link.replace(/^t3code:\/\/(?:threads\/|app\/#\/)/, '').split('/');
  if (
    parts.length !== 2 ||
    parts.join('/') === link ||
    !uuid.test(parts[0]) ||
    !/^[A-Za-z0-9_:-]+$/.test(parts[1])
  )
    throw Error('Expected a T3 thread link with an environment ID and thread ID.');
  return { environment: parts[0].toLowerCase(), id: parts[1] };
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

export function selectModel({ providers, settings }, project, { model, provider, effort }) {
  const fallback = project.defaultModelSelection ??
    settings.defaultModelSelection ?? { instanceId: 'codex', model: 'gpt-6.1-sol' };
  const slug = model ?? fallback.model;
  const matches = providers.filter((p) => p.enabled && p.models.some((m) => m.slug === slug));
  const selected =
    matches.find((p) => p.instanceId === (provider ?? fallback.instanceId)) ??
    (!provider && model && matches.length === 1 ? matches[0] : null);
  if (!selected)
    throw Error(
      matches.length && !provider && model
        ? `Model ${slug} is ambiguous; use --provider with one of: ${matches.map((p) => p.instanceId).join(', ')}.`
        : `Model ${slug} is unavailable on the requested provider.`,
    );
  const options =
    selected.instanceId === fallback.instanceId && slug === fallback.model
      ? [...(fallback.options ?? [])]
      : [];
  const descriptor = selected.models
    .find((m) => m.slug === slug)
    .capabilities?.optionDescriptors?.find((o) => o.id === 'reasoningEffort');
  const existing = options.findIndex((o) => o.id === 'reasoningEffort');
  const value =
    effort ?? (existing < 0 && descriptor?.options?.some((o) => o.id === 'medium') ? 'medium' : null);
  if (value) {
    if (!descriptor?.options?.some((o) => o.id === value))
      throw Error(`Unsupported effort for ${slug}: ${value}.`);
    options.splice(existing < 0 ? options.length : existing, existing < 0 ? 0 : 1, {
      id: 'reasoningEffort',
      value,
    });
  }
  return { instanceId: selected.instanceId, model: slug, ...(options.length ? { options } : {}) };
}

async function onHost({ op, environment, id, text, guarded, options = {}, arg }, lines) {
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
  const write = [
    'pin',
    'unpin',
    'settle',
    'rename',
    'send',
    'queue',
    'start',
    'schedule',
    'promote',
    'stop',
  ].includes(op);
  const args = ['auth', 'session', 'issue', '--scope', 'orchestration:read'];
  if (write) args.push('--scope', 'orchestration:operate');
  const token = execFileSync(cli, [...args, '--ttl', '1m', '--label', 'hey-t3', '--token-only'], {
    encoding: 'utf8',
    timeout: 12000,
    stdio: ['ignore', 'pipe', 'ignore'],
  }).trim();
  const headers = { authorization: 'Bearer ' + token, 'x-t3-orchestration-protocol': '2' };
  const rpc = async (tag, payload, guarded = false) => {
    const ws = new WebSocket(new URL('/ws?orchestrationProtocol=2', base), { headers });
    try {
      return await new Promise((resolve, reject) => {
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
                tag,
                payload,
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
            if (m.requestId === '1')
              m.exit?._tag === 'Success'
                ? resolve(m.exit.value)
                : reject(Error(m.exit?.cause?.[0]?.error?.message ?? `T3 rejected ${tag}.`));
          } catch (e) {
            reject(e);
          }
        };
        AbortSignal.timeout(10000).addEventListener('abort', fail, { once: true });
      });
    } finally {
      ws.close();
    }
  };
  const link = (id) => `t3code://threads/${environment}/${id}`;
  if (['list', 'project', 'projects', 'start', 'search'].includes(op)) {
    const shell = await get(
      '/api/orchestration/shell' +
        (options.all || op === 'search' ? '' : '?omitIdleSubagents=1&omitOlderSettled=1'),
      headers,
    );
    if (op === 'projects') return shell.projects;
    const key = ['start', 'project'].includes(op) ? arg : options.project;
    const projects = shell.projects.filter((p) => [p.id, p.title, p.workspaceRoot].includes(key));
    if (key !== undefined && projects.length !== 1)
      throw Error('Project missing or ambiguous; use an ID from hey-t3 projects.');
    const project = projects[0];
    if (op === 'project') return project;
    const threads = [...shell.threads, ...(shell.archivedThreads ?? [])];
    if (op === 'list')
      return (options.all ? threads : shell.threads)
        .filter(
          (t) =>
            (options.all || t.lineage?.relationshipToParent !== 'subagent') &&
            (!project || t.projectId === project.id),
        )
        .map((t) => ({ ...t, link: link(t.id) }));
    if (op === 'search') {
      const result = await rpc('orchestration.searchThreads', {
        query: arg,
        ...(project ? { projectIds: [project.id] } : {}),
      });
      const query = arg.toLowerCase();
      const matches = new Map();
      for (const t of threads) {
        if (project && project.id !== t.projectId) continue;
        const fields = [
          t.title,
          t.branch,
          ...(t.pullRequests ?? []).flatMap((pr) => [pr.url, '#' + pr.number]),
        ];
        const snippet = fields.find((value) => value?.toLowerCase().includes(query));
        if (snippet) matches.set(t.id, { threadId: t.id, projectId: t.projectId, snippet, link: link(t.id) });
      }
      for (const m of result.matches) {
        if (!matches.has(m.threadId)) matches.set(m.threadId, { ...m, link: link(m.threadId) });
      }
      return [...matches.values()];
    }
    const config = await rpc('server.getConfig', {});
    const modelSelection = selectModel(config, project, options);
    const result = await rpc('orchestration.launchThread', {
      commandId: randomUUID(),
      threadId: randomUUID(),
      projectId: project.id,
      title: options.title ?? (text?.trim().split('\n')[0] || 'New thread'),
      modelSelection,
      runtimeMode: config.settings.defaultRuntimeMode,
      interactionMode: 'default',
      workspaceStrategy: options.worktree
        ? {
            type: 'worktree',
            baseRef: options.worktree,
            ...(options.branch ? { branch: options.branch } : {}),
          }
        : { type: 'root' },
      ...(text === undefined ? {} : { initialMessage: { text, attachments: [] } }),
    });
    return { link: link(result.threadId), modelSelection: result.projection.thread.modelSelection };
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
  if (op === 'schedule') {
    return rpc('scheduledTasks.upsert', {
      commandId: randomUUID(),
      projectId: thread.projectId,
      threadId: id,
      title: text.trim().split('\n')[0],
      prompt: text,
      schedule: JSON.parse(arg),
      enabled: true,
      modelSelection: thread.modelSelection,
      runtimeMode: thread.runtimeMode,
      interactionMode: thread.interactionMode,
      workspaceStrategy: thread.worktreePath
        ? { type: 'existing_worktree', worktreePath: thread.worktreePath }
        : { type: 'root' },
    });
  }
  if (op === 'promote' || op === 'stop') {
    const active = async () => {
      const shell = await get('/api/orchestration/shell', headers);
      const record = [...shell.threads, ...(shell.archivedThreads ?? [])].find((t) => t.id === id);
      if (!record) throw Error('Could not verify the active run.');
      return record.activeRunId;
    };
    const runId = await active();
    if (!runId) {
      if (op === 'stop') return { threadId: id, stopped: true };
      throw Error('No active run to steer.');
    }
    await rpc('orchestration.dispatchCommand', {
      commandId: randomUUID(),
      threadId: id,
      ...(op === 'stop'
        ? { type: 'run.interrupt', runId, holdQueue: true }
        : { type: 'queued-message.promote-to-steer', queuedRunId: arg, targetRunId: runId }),
    });
    if (op === 'promote')
      return { threadId: id, queuedRunId: arg, targetRunId: runId, delivery: 'promotion_requested' };
    const activeRunId = await active();
    return { threadId: id, runId, stopped: activeRunId !== runId, activeRunId, queueHeld: true };
  }
  const command = desiredCommand(op, thread, text);
  if (!command) return thread;
  await rpc('orchestration.dispatchCommand', command, guarded);

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
    return { ...receipt, delivery, ...(delivery === 'queued' && run ? { queuedRunId: run.id } : {}) };
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
  const op = args.shift();
  const envOps = ['start', 'projects', 'project', 'search', 'list'];
  const flags =
    {
      start: ['env', 'title', 'worktree', 'branch', 'model', 'provider', 'effort'],
      list: ['env', 'project', 'all'],
      projects: ['env'],
      project: ['env'],
      search: ['env', 'project'],
    }[op] ?? [];
  // Existing thread commands accept literal message text, including leading dashes.
  const { values: options, positionals } = envOps.includes(op)
    ? parseArgs({
        args,
        allowPositionals: true,
        options: Object.fromEntries(flags.map((f) => [f, { type: f === 'all' ? 'boolean' : 'string' }])),
      })
    : { values: {}, positionals: args };
  const [arg, value, third] = positionals;
  const counts = {
    projects: [0],
    project: [1],
    start: [1, 2],
    search: [1],
    list: [0, 1],
    schedule: [3],
    promote: [2],
    stop: [1],
    show: [1],
    pull: [1],
    'catch-up': [1],
    pin: [1],
    unpin: [1],
    settle: [1],
    rename: [2],
    send: [2],
    queue: [2],
  };
  if (
    !Object.hasOwn(counts, op) ||
    !counts[op].includes(positionals.length) ||
    (options.branch && !options.worktree) ||
    (op === 'list' && arg && options.env) ||
    (guarded && !['pin', 'unpin', 'settle', 'rename', 'send', 'queue'].includes(op))
  )
    throw Error(usage);
  const { environment, id } = envOps.includes(op)
    ? { environment: (options.env ?? (op === 'list' ? arg : null) ?? localID()).toLowerCase() }
    : target(arg);
  if (!uuid.test(environment)) throw Error('Invalid environment ID.');
  let text =
    op === 'schedule' ? third : ['start', 'send', 'queue', 'rename'].includes(op) ? value : undefined;
  if (text === '-' && op !== 'rename') text = readFileSync(0, 'utf8');
  if (text !== undefined && !text.trim()) throw Error('Message or title cannot be empty.');
  const input = {
    op,
    environment,
    id,
    text,
    guarded,
    options,
    arg: ['schedule', 'promote'].includes(op) ? value : arg,
  };

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
