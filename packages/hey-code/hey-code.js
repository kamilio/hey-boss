import { marked } from 'marked';
import DOMPurify from 'dompurify';
import { printable } from './transcript.js';

const active = status => ['running', 'starting', 'queued', 'waiting', 'preparing'].includes(status);
const node = (tag, className, text) => {
  const el = document.createElement(tag);
  if (className) el.className = className;
  if (text !== undefined) el.textContent = text;
  return el;
};
const button = (label, fn, className = '') => {
  const el = node('button', className, label);
  el.type = 'button'; el.addEventListener('click', fn);
  return el;
};
const markdown = (el, text) => {
  el.innerHTML = DOMPurify.sanitize(marked.parse(text ?? '', { async: false }), {
    ALLOWED_TAGS: ['p', 'br', 'hr', 'strong', 'em', 'del', 'blockquote', 'pre', 'code',
      'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'ul', 'ol', 'li', 'table', 'thead', 'tbody', 'tr', 'th', 'td', 'a'],
    ALLOWED_ATTR: ['href', 'title', 'start', 'align'],
  });
  for (const a of el.querySelectorAll('a')) {
    a.target = '_blank'; a.rel = 'noopener noreferrer';
  }
};
const key = chat => `hey-code:draft:${chat.provider}:${chat.id}`;

/** A chat window. The orchestrator owns sessions, persistence and execution. */
export class HeyCode extends HTMLElement {
  #chat;
  #actions = {};
  #rows = new Map();
  #requests = new Map();
  #children = new Map();
  #drafts = new Map();
  #pending = new Set();
  #generation = 0;
  #goalSignature;
  #following = true;

  constructor() {
    super();
    const root = this.attachShadow({ mode: 'open' });
    const css = node('link'); css.rel = 'stylesheet'; css.href = new URL('./hey-code.css', import.meta.url).href;
    this.header = node('header');
    this.headingTitle = node('h1'); this.subtitle = node('span', 'subtitle');
    const heading = node('div', 'heading'); heading.append(this.headingTitle, this.subtitle);
    this.state = node('span', 'status'); this.state.setAttribute('role', 'status');
    this.header.append(heading, this.state);
    this.scroll = node('main', 'scroll'); this.scroll.tabIndex = 0; this.scroll.setAttribute('aria-label', 'Conversation');
    this.content = node('div', 'content');
    this.older = button('Load earlier messages', () => this.#act('older', 'loadOlder', {}), 'older');
    this.timeline = node('div', 'timeline');
    this.empty = node('div', 'empty', 'Start a conversation');
    this.subagents = node('section', 'subagents'); this.subagents.setAttribute('aria-label', 'Subagents');
    this.content.append(this.older, this.empty, this.timeline, this.subagents);
    this.scroll.append(this.content);
    this.bottom = node('div', 'bottom');
    this.jump = button('↓ Latest', () => this.#bottom(), 'jump'); this.jump.hidden = true;
    this.goalPanel = node('section', 'goal'); this.goalPanel.setAttribute('aria-label', 'Goal');
    this.requestPanel = node('section', 'requests'); this.requestPanel.setAttribute('aria-label', 'Agent requests');
    this.error = node('div', 'error'); this.error.setAttribute('role', 'alert'); this.error.hidden = true;
    this.form = node('form', 'composer');
    this.input = node('textarea'); this.input.rows = 2; this.input.placeholder = 'Ask anything…';
    this.input.setAttribute('aria-label', 'Message');
    this.input.addEventListener('input', () => { this.#saveDraft(); this.#controls(); this.#resize(); });
    this.input.addEventListener('keydown', e => {
      if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && e.keyCode !== 229) {
        e.preventDefault(); this.#send();
      }
    });
    this.form.addEventListener('submit', e => { e.preventDefault(); this.#send(); });
    const controls = node('div', 'controls');
    this.model = node('span', 'model');
    this.delivery = node('select'); this.delivery.setAttribute('aria-label', 'Reply delivery');
    this.stop = button('Stop', () => this.#act('stop', 'stop', {}), 'stop');
    this.send = node('button', 'send', '↑'); this.send.type = 'submit'; this.send.setAttribute('aria-label', 'Send message');
    controls.append(this.model, this.delivery, this.stop, this.send);
    this.hint = node('div', 'hint');
    this.form.append(this.input, controls);
    const dock = node('div', 'dock'); dock.append(this.goalPanel, this.requestPanel, this.error, this.form, this.hint);
    this.bottom.append(this.jump, dock);
    root.append(css, this.header, this.scroll, this.bottom);
    this.scroll.addEventListener('scroll', () => {
      this.#following = this.scroll.scrollHeight - this.scroll.scrollTop - this.scroll.clientHeight < 80;
      this.jump.hidden = this.#following;
    });
  }

  get actions() { return this.#actions; }
  set actions(value) {
    if (value === this.#actions) return;
    this.#actions = value ?? {}; this.#goalSignature = undefined; this.#render();
  }
  get chat() { return this.#chat; }
  set chat(value) {
    if (!value?.id || !['codex', 'claude'].includes(value.provider)) throw Error('hey-code requires a chat ID and Codex or Claude provider.');
    if (!this.#chat || key(value) !== key(this.#chat)) {
      this.#saveDraft(); this.#generation++;
      this.#pending.clear(); this.#rows.clear(); this.#requests.clear(); this.#children.clear();
      this.timeline.replaceChildren(); this.requestPanel.replaceChildren(); this.subagents.replaceChildren(); this.goalPanel.replaceChildren();
      this.#goalSignature = undefined; this.#following = true;
      this.error.hidden = true;
      try { this.input.value = this.#drafts.get(key(value)) ?? sessionStorage.getItem(key(value)) ?? ''; }
      catch { this.input.value = this.#drafts.get(key(value)) ?? ''; }
    }
    this.#chat = value;
    this.#render();
  }

  #saveDraft() {
    if (!this.#chat) return;
    const id = key(this.#chat), value = this.input.value;
    this.#drafts.set(id, value);
    try { value ? sessionStorage.setItem(id, value) : sessionStorage.removeItem(id); } catch { /* Storage can be disabled. */ }
  }
  #resize() { this.input.style.height = 'auto'; this.input.style.height = `${Math.min(this.input.scrollHeight, 220)}px`; }
  #bottom() { this.#following = true; this.scroll.scrollTop = this.scroll.scrollHeight; this.jump.hidden = true; }

  async #act(lock, action, payload, accepted) {
    const fn = this.#actions[action], chat = this.#chat, generation = this.#generation;
    if (!fn || !chat || this.#pending.has(lock) || (chat.readOnly && !['loadOlder', 'openSubagent'].includes(action))) return;
    const oldTop = this.scroll.scrollTop, oldHeight = this.scroll.scrollHeight;
    this.#pending.add(lock); this.error.hidden = true; this.#controls();
    try {
      await fn({ threadId: chat.id, ...payload });
      accepted?.(generation === this.#generation);
      if (generation !== this.#generation) return;
      if (action === 'loadOlder') this.scroll.scrollTop = oldTop + this.scroll.scrollHeight - oldHeight;
    } catch (error) {
      if (generation === this.#generation) {
        this.error.textContent = error?.message ?? String(error); this.error.hidden = false;
      }
    } finally {
      if (generation === this.#generation) { this.#pending.delete(lock); this.#controls(); }
    }
  }

  #send() {
    if (!this.#chat || this.send.disabled) return;
    const draft = this.input.value;
    const draftKey = key(this.#chat);
    const delivery = active(this.#chat.status) ? this.delivery.value : 'send';
    this.#act('send', 'send', { text: draft.trim(), delivery }, () => {
      // Preserve text typed while a slow send was being accepted.
      if (key(this.#chat) === draftKey && this.input.value === draft) {
        this.input.value = ''; this.#saveDraft(); this.#resize(); this.input.focus();
      } else if (this.#drafts.get(draftKey) === draft) {
        this.#drafts.delete(draftKey);
        try { sessionStorage.removeItem(draftKey); } catch { /* Storage can be disabled. */ }
      }
    });
  }

  #controls() {
    const chat = this.#chat;
    if (!chat) return;
    const writable = !chat.readOnly;
    this.input.disabled = !writable || !this.#actions.send;
    this.send.disabled = this.input.disabled || !this.input.value.trim() || this.#pending.has('send');
    this.send.textContent = this.#pending.has('send') ? '…' : '↑';
    this.stop.hidden = !active(chat.status) || !this.#actions.stop || !writable;
    this.stop.disabled = this.#pending.has('stop');
    this.older.disabled = this.#pending.has('older');
    this.delivery.hidden = !active(chat.status) || !writable || !this.#actions.send;
    const options = chat.canSteer ? ['steer', 'queue'] : ['queue'];
    if ([...this.delivery.options].map(o => o.value).join() !== options.join()) {
      const selected = this.delivery.value;
      this.delivery.replaceChildren(...options.map(value => new Option(value === 'steer' ? 'Send now' : 'Queue next turn', value)));
      if (options.includes(selected)) this.delivery.value = selected;
    }
    this.hint.textContent = chat.readOnly || (!this.#actions.send ? 'Read-only conversation' : active(chat.status)
      ? (chat.canSteer ? 'Send now to steer, or queue for the next turn' : 'Your reply will start after this turn')
      : 'Enter to send · Shift + Enter for a new line');
    for (const el of this.shadowRoot.querySelectorAll('[data-lock]')) {
      el.disabled = !writable || this.#pending.has(el.dataset.lock) || !this.#actions[el.dataset.action];
    }
  }

  #render() {
    const chat = this.#chat;
    if (!chat) return;
    const follow = this.#following;
    this.headingTitle.textContent = chat.title || 'Untitled conversation';
    this.subtitle.textContent = chat.provider === 'codex' ? 'Codex' : 'Claude';
    this.model.textContent = chat.model || this.subtitle.textContent;
    this.state.textContent = chat.status; this.state.dataset.active = active(chat.status);
    this.older.hidden = !chat.hasMore || !this.#actions.loadOlder;
    this.empty.hidden = chat.items.length > 0;
    const ids = new Set();
    let previous = null;
    for (const item of chat.items) {
      if (ids.has(item.id)) continue;
      ids.add(item.id);
      let row = this.#rows.get(item.id);
      if (!row) { row = this.#item(item); this.#rows.set(item.id, row); }
      const expected = previous ? previous.nextSibling : this.timeline.firstChild;
      if (expected !== row.el) this.timeline.insertBefore(row.el, expected);
      previous = row.el;
      if (row.value !== item) {
        const signature = JSON.stringify(item);
        if (signature !== row.signature) { row.update(item); row.signature = signature; }
        row.value = item;
      }
    }
    for (const [id, row] of this.#rows) if (!ids.has(id)) { row.el.remove(); this.#rows.delete(id); }
    this.#renderGoal(); this.#renderRequests(); this.#renderSubagents(); this.#controls();
    if (follow && !this.#pending.has('older')) this.#bottom();
  }

  #item(item) {
    if (item.type === 'message') {
      const el = node('article', `message ${item.role === 'user' ? 'user' : 'assistant'}`);
      el.setAttribute('aria-label', item.role === 'user' ? 'You' : 'Assistant');
      const body = node('div', 'markdown'); el.append(body);
      return { el, update: value => {
        if (value.role === 'user') body.textContent = value.text ?? '';
        else markdown(body, value.text);
        el.dataset.streaming = !!value.streaming;
      } };
    }
    const el = node('details', 'activity');
    const summary = node('summary'), label = node('span'), status = node('span', 'activity-status');
    summary.append(label, status);
    const body = node('div', 'activity-body'); el.append(summary, body);
    let current = item;
    const fill = () => {
      body.replaceChildren();
      if (current.type === 'reasoning') { body.className = 'activity-body markdown'; markdown(body, current.text); }
      else {
        for (const [title, value] of [['Input', current.input], ['Output', current.output]]) {
          if (value === undefined) continue;
          body.append(node('h3', '', title), node('pre', '', printable(value)));
        }
      }
    };
    el.addEventListener('toggle', () => { if (el.open) fill(); });
    return { el, update: value => {
      current = value; label.textContent = value.type === 'reasoning' ? 'Thinking' : value.name ?? 'Tool';
      status.textContent = value.status ?? ''; status.dataset.status = value.status;
      if (el.open) fill();
    } };
  }

  #lock(el, lock, action) { el.dataset.lock = lock; el.dataset.action = action; return el; }
  #renderGoal() {
    const chat = this.#chat;
    const signature = JSON.stringify([chat.goal, !!this.#actions.goal, chat.readOnly]);
    if (signature === this.#goalSignature) return;
    // A live usage/status update must not discard an objective being edited.
    if (this.goalPanel.querySelector('form')) return;
    this.#goalSignature = signature;
    this.goalPanel.replaceChildren(); this.goalPanel.hidden = !chat.goal && !this.#actions.goal;
    const edit = () => {
      const form = node('form', 'goal-editor');
      const input = node('textarea'); input.rows = 2; input.value = this.#chat.goal?.objective ?? '';
      input.placeholder = 'What should the agent accomplish?'; input.setAttribute('aria-label', 'Goal objective'); input.required = true;
      const save = this.#lock(node('button', 'primary', 'Start goal'), 'goal', 'goal'); save.type = 'submit';
      const cancel = button('Cancel', () => { form.remove(); this.#goalSignature = undefined; this.#renderGoal(); });
      form.append(input, save, cancel);
      form.addEventListener('submit', e => {
        e.preventDefault(); if (!input.value.trim()) return;
        this.#act('goal', 'goal', { objective: input.value.trim(), status: 'active' }, current => {
          if (!current) return;
          form.remove(); this.#goalSignature = undefined; this.#renderGoal();
        });
      });
      this.goalPanel.replaceChildren(form); this.#controls(); input.focus();
    };
    if (!chat.goal) {
      this.goalPanel.append(this.#lock(button('◎ Set a goal', edit, 'set-goal'), 'goal', 'goal')); return;
    }
    const row = node('div', 'goal-row'), lead = node('div', 'goal-copy');
    lead.append(node('span', 'eyebrow', `◎ Goal · ${chat.goal.status}`), node('div', 'objective', chat.goal.objective));
    if (chat.goal.tokensUsed !== undefined) lead.append(node('span', 'usage', `${chat.goal.tokensUsed.toLocaleString()}${chat.goal.tokenBudget ? ' / ' + chat.goal.tokenBudget.toLocaleString() : ''} tokens`));
    row.append(lead);
    if (this.#actions.goal && !chat.readOnly) {
      const change = status => this.#act('goal', 'goal', { objective: this.#chat.goal.objective, status });
      row.append(this.#lock(button(chat.goal.status === 'active' ? 'Pause' : 'Resume', () => change(this.#chat.goal.status === 'active' ? 'paused' : 'active')), 'goal', 'goal'));
      if (chat.goal.status !== 'complete') row.append(this.#lock(button('Complete', () => change('complete')), 'goal', 'goal'));
      row.append(this.#lock(button('Edit', edit), 'goal', 'goal'));
    }
    this.goalPanel.append(row);
  }

  #renderRequests() {
    const ids = new Set();
    for (const request of this.#chat.requests ?? []) {
      ids.add(request.id);
      if (this.#requests.has(request.id)) continue;
      const form = node('form', 'request'); form.append(node('h2', '', request.title));
      const lock = `request:${request.id}`;
      if (request.payload !== undefined) {
        const details = node('details'); details.append(node('summary', '', 'Details'), node('pre', '', printable(request.payload))); form.append(details);
      }
      if (request.kind === 'approval') {
        for (const [label, allow] of [['Allow', true], ['Deny', false]]) form.append(this.#lock(button(label, () => this.#act(lock, 'respond', { requestId: request.id, allow })), lock, 'respond'));
      } else {
        const inputs = [];
        for (const question of request.questions ?? [{ id: 'answer', question: 'Your answer' }]) {
          const label = node('fieldset'), input = node('textarea'); input.rows = 2; input.required = true;
          label.append(node('legend', '', question.question)); input.setAttribute('aria-label', question.question);
          if (question.options?.length) {
            const choices = node('div', 'choices');
            for (const option of question.options) {
              const choice = button(option.label, () => { input.value = option.label; input.focus(); });
              choice.title = option.description ?? ''; choices.append(choice);
            }
            label.append(choices);
          }
          label.append(input); form.append(label); inputs.push([question.id, input]);
        }
        const submit = this.#lock(node('button', 'primary', 'Submit answer'), lock, 'respond'); submit.type = 'submit'; form.append(submit);
        form.addEventListener('submit', e => {
          e.preventDefault();
          this.#act(lock, 'respond', { requestId: request.id, answers: Object.fromEntries(inputs.map(([id, input]) => [id, input.value])) });
        });
      }
      this.requestPanel.append(form); this.#requests.set(request.id, form);
    }
    for (const [id, el] of this.#requests) if (!ids.has(id)) { el.remove(); this.#requests.delete(id); }
    this.requestPanel.hidden = !ids.size;
  }

  #renderSubagents() {
    const ids = new Set();
    for (const child of this.#chat.subagents ?? []) {
      if (child.id === this.#chat.id) continue;
      ids.add(child.id);
      let row = this.#children.get(child.id);
      if (!row) {
        const el = node('details', 'subagent'), summary = node('summary');
        const title = node('span'), status = node('span', 'activity-status'); summary.append(node('span', 'branch', '⑂'), title, status);
        const body = node('div', 'child-body'); el.append(summary, body); this.subagents.append(el);
        row = { el, title, status, body }; this.#children.set(child.id, row);
        el.addEventListener('toggle', () => {
          if (el.open && !row.chat && this.#actions.openSubagent)
            this.#act(`child:${child.id}`, 'openSubagent', { subagentId: child.id });
        });
      }
      row.title.textContent = child.title; row.status.textContent = child.status;
      if (child.chat) {
        if (!row.chat) { row.chat = document.createElement('hey-code'); row.body.replaceChildren(row.chat); }
        row.chat.actions = this.#actions; row.chat.chat = child.chat;
      } else if (!row.chat) row.body.textContent = this.#actions.openSubagent ? 'Open to load this subagent’s conversation.' : 'The host has not supplied this subagent’s conversation.';
    }
    for (const [id, row] of this.#children) if (!ids.has(id)) { row.el.remove(); this.#children.delete(id); }
    this.subagents.hidden = !ids.size;
  }
}

customElements.define('hey-code', HeyCode);
