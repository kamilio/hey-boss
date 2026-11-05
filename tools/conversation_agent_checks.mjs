import assert from 'node:assert/strict';
import {mkdtemp, mkdir, readFile, writeFile, rm, stat, symlink} from 'node:fs/promises';
import {resolve, join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {spawn} from 'node:child_process';
import {setTimeout as delay} from 'node:timers/promises';
import {createInterface} from 'node:readline';

const {createConversation, conversationRoot, interruptedHistory} = await import(pathToFileURL(resolve(process.argv[2], 'conversation-agent.mjs')));
const root = await mkdtemp(resolve('out/conversation-check-'));
try {
  await mkdir(join(root, '.hey-boss'), {recursive: true});
  assert.equal(await conversationRoot(root), join(root, 'Workspace/root'));
  await writeFile(join(root, '.hey-boss/config.yaml'), 'conversation:\n  root: ~/work\n');
  assert.equal(await conversationRoot(root), join(root, 'work'));
  await mkdir(join(root, 'work/.agents/skills/notes'), {recursive: true});
  await writeFile(join(root, 'work/AGENTS.md'), 'Preserve existing notes.');
  await writeFile(join(root, 'work/.agents/skills/notes/SKILL.md'), '---\nname: notes\ndescription: Organize notes\n---\nPrivate skill body.');
  const events = [], requests = [];
  let response = 0, planned;
  const fakeFetch = async (_url, init) => {
    const body = JSON.parse(init.body); requests.push(body);
    const calls = planned?.shift() ?? (response++ === 0 ? [{type: 'function_call', id: 'fc_1', call_id: 'call_1', name: 'batch', arguments: JSON.stringify({commands: [
      {command: 'printf one > one.txt; cat one.txt', description: 'Creating the first note.'},
      {command: 'printf two > two.txt; cat two.txt', description: 'Creating the second note.'},
      {command: 'echo failed >&2; exit 7', description: 'Checking a problem.'},
    ]})}] : [{type: 'message', id: 'msg_1', role: 'assistant', status: 'completed', content: [{type: 'output_text', text: 'Both notes are ready.', annotations: []}]}]);
    const wire = [{type: 'response.created', response: {id: 'response_' + response, status: 'in_progress', output: []}}];
    if (calls[0].type === 'function_call') {
      wire.push({type: 'response.output_item.added', output_index: 0, item: {...calls[0], arguments: ''}});
    } else {
      wire.push({type: 'response.output_item.added', output_index: 0, item: {...calls[0], content: []}});
      wire.push({type: 'response.content_part.added', output_index: 0, content_index: 0, part: {type: 'output_text', text: '', annotations: []}});
      wire.push({type: 'response.output_text.delta', output_index: 0, content_index: 0, delta: calls[0].content[0].text});
    }
    calls.forEach(item => wire.push({type: 'response.output_item.done', output_index: 0, item}));
    wire.push({type: 'response.completed', response: {id: 'response_' + response, status: 'completed', output: calls, usage: {input_tokens: 100, output_tokens: 20}}});
    return new Response(wire.map(event => 'data: ' + JSON.stringify(event) + '\n\n').join(''), {headers: {'content-type': 'text/event-stream'}});
  };
  const statePath = join(root, 'session.json');
  const run = await createConversation({home: root, statePath, emit: event => events.push(event), fetchImpl: fakeFetch});
  await run({turn: 'Create two notes and check a failure.'}, new AbortController().signal);
  assert.equal(await readFile(join(root, 'work/one.txt'), 'utf8'), 'one');
  assert.equal(await readFile(join(root, 'work/two.txt'), 'utf8'), 'two');
  assert.equal(events.filter(e => e.ToolStarted).length, 3);
  assert.equal(events.filter(e => e.ToolCompleted?.failed).length, 1);
  assert.equal(events.at(-1).TurnCompleted.status, 'Completed');
  const serialized = JSON.stringify(requests[0]);
  assert.ok(serialized.includes('Be concise, humane, and speak simply.'));
  assert.ok(serialized.includes('Preserve existing notes.') && serialized.includes('Organize notes'));
  assert.ok(!serialized.includes('Private skill body.'));
  assert.equal(requests[0].store, false);
  assert.deepEqual(requests[0].tools.map(t => t.name), ['shell', 'batch']);
  assert.ok(JSON.stringify(requests[1]).includes('function_call_output'));
  assert.equal((await stat(statePath)).mode & 0o777, 0o600);
  const restored = await createConversation({home: root, statePath, emit: event => events.push(event), fetchImpl: fakeFetch});
  await restored({turn: 'What did you create?'}, new AbortController().signal);
  assert.ok(JSON.stringify(requests.at(-1)).includes('one.txt'));
  assert.equal(events.filter(e => e.ToolStarted).length, 3, 'Resume never reexecutes old commands');
  assert.equal(interruptedHistory([{role: 'assistant', content: '', tool_calls: [{id: 'pending', function: {name: 'shell', arguments: '{}'}}]}])[1].toolCallId, 'pending');
  console.log('PASS real Poe loop: regular shell, batch failure isolation, root config, AGENTS.md, lazy skills, history/resume, file permissions');
  const tool = (name, args) => [{type: 'function_call', id: 'fc_test', call_id: 'call_test', name, arguments: JSON.stringify(args)}];
  const reply = text => [{type: 'message', id: 'msg_test', role: 'assistant', status: 'completed', content: [{type: 'output_text', text, annotations: []}]}];
  // A GUI launch has a sparse PATH. Login and interactive startup files must
  // supply the same commands and functions as the user's terminal.
  const shellHome = join(root, 'shell-home');
  await mkdir(shellHome);
  await writeFile(join(shellHome, '.zprofile'), 'export CONVERSATION_LOGIN_READY=yes\n');
  await writeFile(join(shellHome, '.zshrc'), 'wire_fixture() { printf "terminal-%s" "$CONVERSATION_LOGIN_READY"; }\n');
  await writeFile(join(root, '.hey-boss/config.yaml'), 'conversation:\n  root: ~/work\n  shell: /bin/zsh\n');
  const oldZdotdir = process.env.ZDOTDIR, oldPath = process.env.PATH;
  process.env.ZDOTDIR = shellHome; process.env.PATH = '/usr/bin:/bin';
  try {
    const terminal = await createConversation({home: root, statePath: join(root, 'shell.json'), emit: e => events.push(e), fetchImpl: fakeFetch});
    planned = [tool('shell', {command: `wire_fixture; printf '%s' ' quoted $HOME; $(false)'`, description: 'Checking terminal commands.'}), reply('Ready.')];
    await terminal({turn: 'Check the terminal.'}, new AbortController().signal);
    assert.equal(events.findLast(e => e.ToolCompleted).ToolCompleted.output.aggregatedOutput, 'terminal-yes quoted $HOME; $(false)');
  } finally {
    if (oldZdotdir === undefined) delete process.env.ZDOTDIR; else process.env.ZDOTDIR = oldZdotdir;
    process.env.PATH = oldPath;
  }
  console.log('PASS configured login shell loads terminal startup files and preserves command quoting');
  planned = [tool('batch', {commands: [{command: 'touch must-not-run', description: 'Creating a file.'}, {command: 'true'}]}), reply('Invalid description.')];
  await run({turn: 'Invalid batch.'}, new AbortController().signal);
  await assert.rejects(stat(join(root, 'work/must-not-run')), {code: 'ENOENT'});
  console.log('PASS required descriptions: reject the whole invalid batch before starting commands');
  const controller = new AbortController();
  const cancel = await createConversation({home: root, statePath: join(root, 'cancel.json'), fetchImpl: fakeFetch,
    emit: event => { if (event.ToolStarted) void (async () => {
      for (let i = 0; i < 100; i++) {
        if (await stat(join(root, 'work/waiting')).catch(() => null)) break;
        await delay(20);
      }
      controller.abort();
    })(); }});
  planned = [tool('shell', {command: 'printf started > waiting; sleep 30; printf orphan > orphan', description: 'Waiting for a result.'})];
  const before = Date.now();
  await assert.rejects(cancel({turn: 'Start then cancel.'}, controller.signal));
  assert.ok(Date.now() - before < 4000, 'Cancel kills the shell and its child processes promptly');
  assert.equal(await readFile(join(root, 'work/waiting'), 'utf8'), 'started');
  await assert.rejects(stat(join(root, 'work/orphan')), {code: 'ENOENT'});
  console.log('PASS cancellation kills a running shell and descendants');
  const longHistory = Array.from({length: 4}, (_, i) => ({role: 'user', content: 'Earlier turn ' + i + ' ' + 'x'.repeat(60000)}));
  for (const empty of [false, true]) {
    const compactPath = join(root, empty ? 'compact-failed.json' : 'compact.json');
    const compact = await createConversation({home: root, statePath: compactPath, emit: () => {}, fetchImpl: fakeFetch});
    planned = [reply('All done.'), reply(empty ? '' : 'Earlier tasks and results are retained here.')];
    const result = compact({turn: 'Continue.', history: longHistory}, new AbortController().signal);
    if (empty) await assert.rejects(result, /empty summary/); else await result;
    const saved = JSON.parse(await readFile(compactPath, 'utf8')).messages;
    assert.equal(saved.some(m => m.name === 'compaction'), !empty);
    assert.equal(saved.filter(m => m.role === 'user').length, empty ? 5 : 3);
  }
  console.log('PASS automatic compaction retains recent turns; empty summary preserves original durable history');
  const mock = join(root, 'provider.mjs');
  // Give the subprocess a deterministic provider without changing its production protocol.
  await writeFile(mock, 'let response = 10, planned; const requests = [];\nglobalThis.fetch = ' + fakeFetch.toString() + ';\n');
  const entry = join(root, 'agent.mjs');
  await symlink(resolve(process.argv[2], 'conversation-agent.mjs'), entry);
  const child = spawn(process.execPath, ['--import', mock, entry, join(root, 'protocol.json')],
    {env: {...process.env, HOME: root}, stdio: ['pipe', 'pipe', 'inherit']});
  const exited = new Promise(resolve => child.on('close', resolve));
  const timeout = setTimeout(() => child.kill(), 10000);
  child.stdin.write(JSON.stringify({turn: 'Immediately after launch.'}) + '\n');
  let completed = 0;
  for await (const line of createInterface({input: child.stdout})) {
    const event = JSON.parse(line);
    assert.equal(event.error, undefined);
    if (event.TurnCompleted) {
      if (++completed === 1) child.stdin.write(JSON.stringify({turn: 'Second turn.'}) + '\n');
      else child.stdin.end();
    }
  }
  clearTimeout(timeout);
  assert.equal(await exited, 0);
  assert.equal(completed, 2, 'The first command sent during initialization must be buffered');
  console.log('PASS actual JSONL subprocess: symlinked launch, immediate first message, second turn and clean EOF');
} finally { await rm(root, {recursive: true, force: true}); }
