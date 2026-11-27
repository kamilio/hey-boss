import { test } from 'node:test';
import assert from 'node:assert/strict';
import { codexTranscript, claudeTranscript, codexThread, runtimeEvent } from './transcript.js';

const chat = () => ({ id: 'thread-a', title: 'Test', provider: 'codex', status: 'idle', items: [] });

test('Codex rollout uses canonical messages, not duplicate event_msg copies', () => {
  const items = codexTranscript([
    { type: 'event_msg', payload: { type: 'user_message', message: 'hello' } },
    { type: 'response_item', payload: { type: 'message', role: 'user', content: [{ type: 'input_text', text: 'hello' }] } },
    { type: 'response_item', payload: { type: 'message', role: 'developer', content: [{ type: 'input_text', text: 'internal instructions' }] } },
    { type: 'response_item', payload: { type: 'function_call', call_id: 'call', name: 'exec_command', arguments: '{"cmd":"pwd"}' } },
    { type: 'response_item', payload: { type: 'function_call_output', call_id: 'call', output: '/work' } },
    { type: 'response_item', payload: { type: 'message', role: 'assistant', content: [{ type: 'output_text', text: 'Done.' }] } },
  ]);
  assert.equal(items.length, 3);
  assert.deepEqual(items[1], { id: 'call', type: 'tool', name: 'exec_command', input: { cmd: 'pwd' }, output: '/work', status: 'completed' });
  assert.equal(items[2].text, 'Done.');
});

test('Claude JSONL deduplicates replayed UUIDs, preserves SDK blocks, excludes forwarded child text', () => {
  const user = { type: 'user', uuid: 'u', message: { content: 'Please check' } };
  const items = claudeTranscript([user, user,
    { type: 'assistant', message: { id: 'same', content: [{ type: 'text', text: 'Checking.' }] } },
    { type: 'assistant', message: { id: 'same', content: [{ type: 'tool_use', id: 't', name: 'Bash', input: { command: 'pwd' } }] } },
    { type: 'user', message: { content: [{ type: 'tool_result', tool_use_id: 't', content: 'denied', is_error: true }] } },
    { type: 'assistant', parent_tool_use_id: 'child', message: { content: [{ type: 'text', text: 'child only' }] } },
  ]);
  assert.equal(items.length, 3);
  assert.equal(items[1].text, 'Checking.');
  assert.equal(items[2].status, 'failed');
  assert.equal(new Set(items.map(i => i.id)).size, 3);
});

test('app-server history renders supported items without file preview surfaces', () => {
  const items = codexThread({ turns: [{ items: [
    { id: 'u', type: 'userMessage', content: [{ type: 'text', text: 'test' }] },
    { id: 'a', type: 'agentMessage', text: 'reply' },
    { id: 'r', type: 'reasoning', summary: ['one', 'two'] },
    { id: 'f', type: 'fileChange', changes: ['secret diff'] },
    { id: 't', type: 'commandExecution', command: 'pwd', aggregatedOutput: '/work', status: 'completed' },
  ] }] });
  assert.deepEqual(items.map(i => i.id), ['u', 'a', 'r', 't']);
  assert.equal(items[3].output, '/work');
});

test('runtime streams replace the partial reply and preserve commentary across tools and turns', () => {
  const initial = chat();
  let state = initial;
  for (const event of [
    { TurnStarted: { id: 'turn' } },
    { TextDelta: { text: 'Check' } }, { TextDelta: { text: 'ing.' } },
    { Message: { role: 'assistant', text: 'Checking.' } },
    { ToolStarted: { id: 't', name: 'Bash', input: 'pwd' } },
    { ToolCompleted: { id: 't', output: '/work', failed: false } },
    { TextDelta: { text: 'Done.' } }, { Message: { role: 'assistant', text: 'Done.' } },
    { TurnCompleted: { status: 'Completed', output: 'Done.' } },
  ]) state = runtimeEvent(state, event);
  assert.equal(initial.items.length, 0, 'input snapshot was mutated');
  assert.deepEqual(state.items.map(i => i.text ?? i.name), ['Checking.', 'Bash', 'Done.']);
  assert.equal(state.status, 'completed');
  assert.ok(state.items.every(i => !i.streaming));
  state = runtimeEvent(state, { TurnStarted: { id: 'two' } });
  state = runtimeEvent(state, { TextDelta: { text: 'Next' } });
  state = runtimeEvent(state, { TurnCompleted: { status: 'Interrupted' } });
  assert.equal(state.items.at(-1).text, 'Next');
  assert.equal(state.status, 'interrupted');
});

test('runtime goals, requests, queued successors and native subagents retain identities', () => {
  let state = chat();
  state = runtimeEvent(state, { Goal: { objective: 'Ship it', status: 'active' } });
  state = runtimeEvent(state, { Approval: { id: 'approval', kind: 'Bash', payload: { command: 'pwd' } } });
  state = runtimeEvent(state, { Input: { id: 'input', payload: { params: { questions: [{ id: 'q', question: 'Which?' }] } } } });
  state = runtimeEvent(state, { RequestCancelled: { id: 'approval' } });
  state = runtimeEvent(state, { Other: { method: 'item/completed', params: { item: { type: 'collabAgentToolCall', receiverThreadIds: ['child'], prompt: 'Check UI', agentsStates: { child: { status: 'completed' } } } } } });
  assert.equal(state.goal.objective, 'Ship it');
  assert.deepEqual(state.requests.map(r => r.id), ['input']);
  assert.equal(state.requests[0].questions[0].id, 'q');
  assert.deepEqual(state.subagents, [{ id: 'child', title: 'Check UI', status: 'completed' }]);
  state = runtimeEvent(state, { TurnCompleted: { status: 'Completed', next_turn: 'next' } });
  assert.equal(state.status, 'running');
  assert.equal(state.requests.length, 0);
});

test('Claude task updates update the same subagent card', () => {
  let state = chat();
  state = runtimeEvent(state, { ToolStarted: { id: 'tool', name: 'Agent', input: { description: 'Review' } } });
  for (const e of [
    { type: 'system', subtype: 'task_started', task_id: 'task', tool_use_id: 'tool' },
    { type: 'system', subtype: 'task_notification', task_id: 'task', status: 'completed' },
  ]) state = runtimeEvent(state, { Other: e });
  assert.deepEqual(state.subagents, [{ id: 'task', title: 'Review', status: 'completed' }]);
});

test('paged transcripts keep fallback IDs stable as earlier records are loaded', () => {
  for (const [adapt, record] of [
    [codexTranscript, { type: 'response_item', payload: { type: 'message', role: 'user', content: [{ type: 'input_text', text: 'hello' }] } }],
    [claudeTranscript, { type: 'user', message: { content: 'hello' } }],
  ]) {
    assert.equal(adapt([record], 10)[0].id, adapt(Array(11).fill(record))[10].id);
    assert.notEqual(adapt([record], 10)[0].id, adapt([record], 0)[0].id);
  }
});

test('interleaved tool events do not duplicate a streamed assistant message', () => {
  let state = runtimeEvent(chat(), { TextDelta: { text: 'Checking.' } });
  const previous = state;
  state = runtimeEvent(state, { ToolStarted: { id: 'tool', name: 'Bash', input: 'pwd' } });
  state = runtimeEvent(state, { Message: { role: 'assistant', text: 'Checking.' } });
  assert.equal(state.items.filter(i => i.type === 'message').length, 1);
  assert.equal(previous.items[0].streaming, true, 'previous snapshot was mutated');
});
