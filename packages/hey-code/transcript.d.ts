import type { Chat, Item } from './hey-code.js';

/** firstRecord is the absolute record index for a page of JSONL history. */
export function codexTranscript(records: Record<string, unknown>[], firstRecord?: number): Item[];
export function claudeTranscript(records: Record<string, unknown>[], firstRecord?: number): Item[];
export function codexThread(thread: { turns?: { items?: Record<string, unknown>[] }[] }): Item[];
export function runtimeEvent(chat: Chat, event: Record<string, unknown>): Chat;
export function printable(value: unknown): string | undefined;
