export interface Item {
  id: string;
  type: 'message' | 'tool' | 'reasoning';
  role?: string;
  text?: string;
  name?: string;
  input?: unknown;
  output?: unknown;
  status?: string;
  streaming?: boolean;
}
export interface Goal {
  objective: string;
  status: string;
  tokensUsed?: number;
  tokenBudget?: number | null;
}
export interface Request {
  id: string;
  kind: 'approval' | 'input';
  title: string;
  payload?: unknown;
  questions?: { id: string; question: string; options?: { label: string; description?: string }[] }[];
}
export interface Chat {
  /** Include the environment/provider in this key when IDs are not globally unique. */
  id: string;
  title: string;
  provider: 'codex' | 'claude';
  model?: string;
  status: string;
  items: Item[];
  goal?: Goal | null;
  requests?: Request[];
  subagents?: { id: string; title: string; status: string; chat?: Chat }[];
  hasMore?: boolean;
  readOnly?: string;
  /** The host must explicitly advertise steering; otherwise active replies queue. */
  canSteer?: boolean;
}
export interface Actions {
  /** Resolve only once the host accepts the operation. Reject with a user-facing error. */
  send?: (input: { threadId: string; text: string; delivery: 'send' | 'steer' | 'queue' }) => Promise<void>;
  stop?: (input: { threadId: string }) => Promise<void>;
  goal?: (input: { threadId: string; objective: string; status: 'active' | 'paused' | 'complete' }) => Promise<void>;
  respond?: (input: { threadId: string; requestId: string; allow?: boolean; answers?: Record<string, string> }) => Promise<void>;
  loadOlder?: (input: { threadId: string }) => Promise<void>;
  openSubagent?: (input: { threadId: string; subagentId: string }) => Promise<void>;
}
export class HeyCode extends HTMLElement {
  chat: Chat;
  actions: Actions;
}
declare global { interface HTMLElementTagNameMap { 'hey-code': HeyCode } }
