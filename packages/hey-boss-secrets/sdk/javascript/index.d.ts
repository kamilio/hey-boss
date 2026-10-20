export type Kind = 'value' | 'bundle' | 'blob' | 'cookies';
export interface Record {
  id: string; name: string; kind: Kind; owner: string; owner_label: string;
  writer: string; writer_label: string; revision: string; access_version: number;
  recipients: string[]; updated_ms: number; deleted: boolean; status: string;
}
export class SecretError extends Error {}
export class Secret {
  constructor(value: Uint8Array);
  expose(): Buffer;
  close(): void;
  toString(): string;
  toJSON(): string;
}
export class Client {
  constructor(binary?: string);
  get(name: string): Promise<{record: Record; secret: Secret}>;
  put(name: string, value: Uint8Array, options?: {kind?: Kind; expected?: string | null}): Promise<Record>;
}
