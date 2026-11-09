import {readFile, mkdir, open, rename, realpath} from 'node:fs/promises';
import {homedir, userInfo} from 'node:os';
import {dirname, isAbsolute, join} from 'node:path';
import {createInterface} from 'node:readline';
import {pathToFileURL} from 'node:url';
import {createConnection} from 'node:net';
import {randomUUID} from 'node:crypto';
import {agent, shellPlugin, memoryPlugin, skillsPlugin, compactionPlugin, openaiResponsesPlugin, parseYaml} from './poe-agent.mjs';

async function optionalFile(path) {
  try { return await readFile(path, 'utf8'); }
  catch (error) { if (error.code === 'ENOENT') return undefined; throw error; }
}

export async function conversationRoot(home) {
  const config = parseYaml(await optionalFile(join(home, '.hey-boss/config.yaml')) ??
    await optionalFile(join(home, '.hey-boss/config.json')) ?? '{}');
  const value = config?.conversation?.root ?? '~/Workspace/root';
  if (typeof value !== 'string' || !value.trim()) throw new Error('conversation.root must be an absolute path or start with ~/.');
  const root = value.startsWith('~/') ? join(home, value.slice(2)) : value;
  if (!isAbsolute(root)) throw new Error('conversation.root must be an absolute path or start with ~/.');
  return root;
}

export async function conversationShell(home) {
  const config = parseYaml(await optionalFile(join(home, '.hey-boss/config.yaml')) ??
    await optionalFile(join(home, '.hey-boss/config.json')) ?? '{}');
  const shell = config?.conversation?.shell ?? userInfo().shell ?? process.env.SHELL ?? '/bin/sh';
  if (typeof shell !== 'string' || !isAbsolute(shell)) throw new Error('conversation.shell must be an absolute path.');
  return shell;
}

const shellQuote = value => "'" + value.replaceAll("'", "'\\''") + "'";

// A saved call may have changed files even if the app stopped before its result arrived.
// Preserve that call, but never execute it again when reopening the conversation.
export function interruptedHistory(messages) {
  const answered = new Set(messages.filter(m => m.role === 'tool').map(m => m.toolCallId));
  return messages.flatMap(message => [message, ...(message.tool_calls ?? [])
    .filter(call => !answered.has(call.id))
    .map(call => ({role: 'tool', name: call.function.name, toolCallId: call.id,
      content: 'Interrupted before the result was saved. The command may have run.'}))]);
}

export async function createConversation({home = homedir(), statePath, emit, fetchImpl = fetch}) {
  const cwd = await conversationRoot(home);
  const loginShell = await conversationShell(home);
  await mkdir(cwd, {recursive: true});
  await mkdir(dirname(statePath), {recursive: true, mode: 0o700});
  const base = (await readFile(new URL('./conversation.md', import.meta.url), 'utf8')).trim();
  const [guidance, shellDescription, batchDescription, windowDescription] = (await readFile(new URL('./conversation-tools.md', import.meta.url), 'utf8')).trim().split('\n\n');
  const stored = await optionalFile(statePath);
  let history = stored === undefined ? undefined : interruptedHistory(JSON.parse(stored).messages);
  async function checkpoint(messages) {
    const snapshot = structuredClone(messages);
    const staging = statePath + '.new';
    const file = await open(staging, 'w', 0o600);
    try { await file.writeFile(JSON.stringify({messages: snapshot})); await file.sync(); }
    finally { await file.close(); }
    await rename(staging, statePath);
    history = snapshot;
  }
  return async function run({turn, history: legacy = []}, signal) {
    const keys = JSON.parse(await optionalFile(join(home, '.hey-boss/proxy/config.access-keys.json')) ?? '{}');
    if (keys.local !== undefined && (typeof keys.local !== 'string' || !keys.local)) throw new Error('Hey Proxy’s local access key is invalid.');
    const shell = shellPlugin({cwd});
    const execute = shell.tools.find(tool => tool.name === 'run_command');
    const commandSchema = {type: 'object', properties: {command: {type: 'string'}, description: {type: 'string'}}, required: ['command', 'description'], additionalProperties: false};
    const validate = args => {
      if (!args || typeof args.command !== 'string' || !args.command.trim() ||
          typeof args.description !== 'string' || !args.description.trim()) throw new Error('command and description must be nonempty strings.');
    };
    async function command(args, ctx) {
      validate(args); ctx.signal.throwIfAborted();
      const id = randomUUID();
      emit({ToolStarted: {id, input: args}});
      try {
        // Interactive shells otherwise ignore TERM and can launch jobs in new
        // process groups, escaping the plugin's cancellation of the command.
        const script = `set +m; trap 'exit 143' TERM;\n${args.command}`;
        const result = await execute.call({command: `exec ${shellQuote(loginShell)} -ilc ${shellQuote(script)}`}, ctx);
        emit({ToolCompleted: {id, output: {...args, aggregatedOutput: result}, failed: false}});
        return result;
      } catch (error) {
        emit({ToolCompleted: {id, output: {...args, aggregatedOutput: error.message}, failed: true}});
        throw error;
      }
    }
    const compact = compactionPlugin();
    const runner = agent({cwd, homeDir: home}).model('gpt-6.1-sol')
      .use(openaiResponsesPlugin({baseUrl: 'http://127.0.0.1:8080/v1', apiKey: keys.local ?? 'local', maxRetries: 0, reasoningEffort: 'low'}))
      .use(memoryPlugin()).use(skillsPlugin({directories: [join(cwd, '.agents/skills'), join(home, '.agents/skills'), join(home, '.config/poe-code/skills')]}))
      .use({name: 'conversation', prompt: ctx => ({...ctx, system: [ctx.system, guidance].join('\n\n')}), hooks: {
        async userPromptSubmit(ctx) { await checkpoint(ctx.messages); emit({AgentAccepted: {root: cwd}}); },
        async preToolUse(ctx) { await checkpoint(ctx.messages); },
        async postToolUse(ctx) {
          await checkpoint([...ctx.messages, {role: 'tool', name: ctx.tool, toolCallId: ctx.intentId, content: ctx.error ?? ctx.result}]);
        },
        async postIteration(ctx) {
          await checkpoint(ctx.messages);
          const message = ctx.messages.findLast(m => m.role === 'assistant');
          if (typeof message?.content === 'string' && message.content) emit({Message: {text: message.content}});
          // Reuse Poe's compaction, but reject empty/aborted summaries before it replaces history.
          await compact.hooks.postIteration({...ctx, complete: async messages => {
            emit({AgentStatus: {text: 'Remembering the conversation…'}});
            const summary = await ctx.complete(messages);
            ctx.signal.throwIfAborted();
            if (!summary.trim()) throw new Error('Could not compact the conversation: empty summary. Your history is saved.');
            return summary;
          }});
          await checkpoint(ctx.messages);
        },
      }})
      .tools({name: 'shell', description: shellDescription, inputSchema: commandSchema, call: command},
        {name: 'batch', description: batchDescription, inputSchema: {type: 'object', properties: {
          commands: {type: 'array', items: commandSchema, minItems: 1, maxItems: 8}}, required: ['commands'], additionalProperties: false},
        async call(args, ctx) {
          if (!Array.isArray(args?.commands) || args.commands.length < 1 || args.commands.length > 8) throw new Error('batch requires 1–8 commands.');
          args.commands.forEach(validate);
          const results = await Promise.allSettled(args.commands.map(args => command(args, ctx)));
          ctx.signal.throwIfAborted();
          return JSON.stringify(results.map((result, i) => ({...args.commands[i], ...(result.status === 'fulfilled' ? {output: result.value} : {error: result.reason.message})})));
        }},
        {name: 'inspect_window', description: windowDescription, inputSchema: {type: 'object', properties: {
          target: {type: 'string', examples: ['active', 'browser', 'Gmail', 'Obsidian', 'Slack', 'cmux']},
          mode: {type: 'string', enum: ['accessibility', 'screenshot', 'both']}},
          required: ['target', 'mode'], additionalProperties: false},
        async call({target, mode}, ctx) {
          ctx.signal.throwIfAborted();
          const id = randomUUID(), input = {command: mode + ': ' + target, description: 'Looking at ' + (target === 'active' ? 'the current window' : target) + '.'};
          emit({ToolStarted: {id, input}});
          const socket = createConnection({path: join(home, 'Library/Application Support/hey-talk/control.sock'), signal: ctx.signal});
          try {
            socket.setTimeout(105000, () => socket.destroy(new Error('Hey Talk did not answer.')));
            socket.write(JSON.stringify({command: 'inspect_window', target, mode}) + '\n');
            let size = 0; const chunks = [];
            for await (const chunk of socket) {
              size += chunk.length; if (size > 8 * 1024 * 1024) throw new Error('Window capture is too large.');
              chunks.push(chunk);
            }
            const result = JSON.parse(Buffer.concat(chunks).toString());
            if (!result.ok) throw new Error(result.error);
            const text = result.source + (result.text ? '\n' + result.text : '');
            emit({ToolCompleted: {id, output: {...input, aggregatedOutput: text + (result.jpeg ? '\nScreenshot attached.' : '')}, failed: false}});
            return [{type: 'text', text}, ...(result.jpeg ? [{type: 'image', mimeType: 'image/jpeg', data: result.jpeg}] : [])];
          } catch (error) {
            const message = ['ENOENT', 'ECONNREFUSED'].includes(error.code) ? 'Start Hey Talk on this Mac to inspect a window.' : error.message;
            emit({ToolCompleted: {id, output: {...input, aggregatedOutput: message}, failed: true}});
            throw new Error(message);
          } finally { socket.destroy(); }
        }});
    try {
      for await (const event of runner.stream(turn, {signal, baseSystemPrompt: base, resume: {messages: interruptedHistory(history ?? legacy)},
        fetch: (url, init) => fetchImpl(url, {...init, body: JSON.stringify({...JSON.parse(init.body), store: false, text: {verbosity: 'low'}})})})) {
        if (event.type === 'message.delta') emit({TextDelta: {text: event.content}});
        if (event.type === 'session.error') throw event.error;
        if (event.type === 'session.complete') {
          await checkpoint(event.result.messages);
          emit({TurnCompleted: {status: 'Completed', output: event.result.output}});
        }
      }
    } finally { await shell.dispose(); }
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(await realpath(process.argv[1])).href) {
  const emit = value => process.stdout.write(JSON.stringify(value) + '\n');
  const controller = new AbortController();
  const input = createInterface({input: process.stdin});
  // Subscribe before async initialization so the first UI message cannot be lost.
  const lines = input[Symbol.asyncIterator]();
  input.on('close', () => controller.abort());
  process.on('SIGTERM', () => { controller.abort(); input.close(); });
  process.on('SIGINT', () => { controller.abort(); input.close(); });
  try {
    const run = await createConversation({statePath: process.argv[2], emit});
    let pending = Promise.resolve();
    for await (const line of lines) {
      const request = JSON.parse(line);
      pending = pending.then(() => run(request, controller.signal)).catch(error => {
        if (!controller.signal.aborted) emit({error: error.message});
      });
    }
    await pending;
  } catch (error) { emit({error: error.message}); input.close(); process.exitCode = 1; }
}
