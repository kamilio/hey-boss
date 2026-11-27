// Native records stay at the host boundary. No filesystem access or session ownership here.
const text = value => typeof value === 'string' ? value : Array.isArray(value)
  ? value.filter(b => ['text', 'input_text', 'output_text'].includes(b.type)).map(b => b.text ?? '').join('\n') : '';
const printable = value => typeof value === 'string' ? value : JSON.stringify(value, null, 2);
const parse = value => { try { return JSON.parse(value); } catch { return value; } };
const agentTool = name => /(?:^|[./_])(?:spawn_agent|Agent|Task|delegate_task)$/.test(name ?? '');

function tool(items, id, name, input) {
  items.push({ id, type: 'tool', name, input, status: 'running' });
}
function result(items, id, output, failed = false) {
  const index = items.findLastIndex(i => i.type === 'tool' && i.id === id);
  if (index >= 0) items[index] = { ...items[index], output, status: failed ? 'failed' : 'completed' };
}

/** Codex rollout JSONL records, including archived sessions. Host supplies parsed records. */
export function codexTranscript(records, firstRecord = 0) {
  const items = [];
  records.forEach((record, index) => {
    if (record.type !== 'response_item') return;
    const p = record.payload, id = p.id ?? `codex:${firstRecord + index}`;
    if (p.type === 'message' && ['user', 'assistant'].includes(p.role)) {
      const body = text(p.content);
      if (body) items.push({ id, type: 'message', role: p.role, text: body });
    } else if (p.type === 'reasoning') {
      const body = (p.summary ?? []).map(s => s.text ?? '').join('\n');
      if (body) items.push({ id, type: 'reasoning', text: body });
    } else if (['function_call', 'custom_tool_call'].includes(p.type)) {
      tool(items, p.call_id ?? id, p.name, parse(p.arguments ?? p.input));
    } else if (['function_call_output', 'custom_tool_call_output'].includes(p.type)) {
      result(items, p.call_id, p.output);
    }
  });
  return items;
}

/** Claude session JSONL or SDK assistant/user records. Child messages remain in their child chat. */
export function claudeTranscript(records, firstRecord = 0) {
  const items = [], seen = new Set();
  records.forEach((record, index) => {
    if (!['user', 'assistant'].includes(record.type) || record.parent_tool_use_id) return;
    const id = record.uuid ?? `claude:${firstRecord + index}`;
    if (record.uuid && seen.has(record.uuid)) return;
    if (record.uuid) seen.add(record.uuid);
    const content = record.message?.content;
    if (typeof content === 'string') items.push({ id, type: 'message', role: record.type, text: content });
    else (content ?? []).forEach((block, n) => {
      if (block.type === 'text') items.push({ id: `${id}:${n}`, type: 'message', role: record.type, text: block.text });
      if (block.type === 'thinking') items.push({ id: `${id}:${n}`, type: 'reasoning', text: block.thinking });
      if (block.type === 'tool_use') tool(items, block.id, block.name, block.input);
      if (block.type === 'tool_result') result(items, block.tool_use_id, block.content, block.is_error);
    });
  });
  return items;
}

/** Codex app-server thread/read response's thread. */
export function codexThread(thread) {
  return (thread.turns ?? []).flatMap(turn => (turn.items ?? []).flatMap(item => {
    const base = { id: item.id, status: item.status };
    if (item.type === 'userMessage') return [{ ...base, type: 'message', role: 'user', text: text(item.content) }];
    if (item.type === 'agentMessage') return [{ ...base, type: 'message', role: 'assistant', text: item.text }];
    if (item.type === 'reasoning') return [{ ...base, type: 'reasoning', text: (item.summary ?? []).join('\n') }];
    if (['commandExecution', 'mcpToolCall', 'dynamicToolCall', 'collabAgentToolCall'].includes(item.type))
      return [{ ...base, type: 'tool', name: item.tool ?? item.type, input: item.arguments ?? item.command ?? item.prompt,
        output: item.aggregatedOutput ?? item.result ?? item.agentsStates }];
    // File changes, document previews, and unrelated metadata have no special surface.
    return [];
  }));
}

/** Apply one serialized hey_boss::agent_runtime::Event. The host owns ordering/deduplication. */
export function runtimeEvent(chat, event) {
  const next = { ...chat, items: [...chat.items],
    requests: [...(chat.requests ?? [])], subagents: [...(chat.subagents ?? [])] };
  const [kind, e] = Object.entries(event)[0] ?? [];
  const stream = () => next.items.findLast(i => i.streaming);
  const update = (item, patch) => { next.items[next.items.indexOf(item)] = { ...item, ...patch }; };
  const finish = () => { const item = stream(); if (item) update(item, { streaming: false }); };
  const message = (role, body, streaming = false) => {
    next.items.push({ id: `runtime:${crypto.randomUUID()}`, type: 'message', role, text: body, streaming });
  };
  switch (kind) {
    case 'TurnStarted': finish(); next.status = 'running'; break;
    case 'TextDelta': {
      const item = stream();
      if (item) update(item, { text: item.text + e.text });
      else message('assistant', e.text, true);
      break;
    }
    case 'Message': {
      const item = stream();
      if (item && e.role === 'assistant') update(item, { text: e.text, streaming: false });
      else message(e.role, e.text);
      break;
    }
    case 'ToolStarted': {
      tool(next.items, e.id, e.name, e.input);
      if (agentTool(e.name) && !next.subagents.some(s => s.id === e.id))
        next.subagents.push({ id: e.id, title: e.input?.description ?? e.input?.task ?? e.input?.message ?? e.name, status: 'running' });
      break;
    }
    case 'ToolCompleted': result(next.items, e.id, e.output, e.failed); break;
    case 'TurnCompleted': {
      finish();
      next.status = e.next_turn ? 'running' : e.status.toLowerCase();
      next.requests = [];
      break;
    }
    case 'Goal': next.goal = e; break;
    case 'Approval': next.requests.push({ id: e.id, kind: 'approval', title: e.kind, payload: e.payload }); break;
    case 'Input': {
      const questions = e.payload?.params?.questions ?? e.payload?.request?.input?.questions;
      next.requests.push({ id: e.id, kind: 'input', title: 'Agent needs your input', payload: e.payload,
        questions: questions?.map((q, i) => ({ ...q, id: q.id ?? q.header ?? String(i) })) });
      break;
    }
    case 'RequestCancelled': next.requests = next.requests.filter(r => r.id !== e.id); break;
    case 'Other': {
      const item = e.params?.item;
      if (e.method === 'item/completed' && item?.type === 'userMessage') message('user', text(item.content));
      if (item?.type === 'collabAgentToolCall') {
        for (const id of item.receiverThreadIds ?? []) {
          const state = item.agentsStates?.[id];
          const child = { id, title: item.prompt || 'Subagent', status: state?.status ?? 'running' };
          const index = next.subagents.findIndex(s => s.id === id);
          if (index < 0) next.subagents.push(child);
          else next.subagents[index] = { ...next.subagents[index], ...child };
        }
      }
      if (e.type === 'system' && e.task_id && ['task_started', 'task_notification', 'task_updated'].includes(e.subtype)) {
        const id = e.task_id;
        const index = next.subagents.findIndex(s => s.id === id || s.id === e.tool_use_id);
        const child = { id, title: e.description ?? next.subagents[index]?.title ?? 'Subagent',
          status: e.status ?? e.patch?.status ?? 'running' };
        if (index < 0) next.subagents.push(child);
        else next.subagents[index] = { ...next.subagents[index], ...child };
      }
      break;
    }
  }
  return next;
}

export { printable };
