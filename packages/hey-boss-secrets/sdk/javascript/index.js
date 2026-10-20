import { spawn } from 'node:child_process';
import { inspect } from 'node:util';

export class SecretError extends Error {
  constructor(code) { super(code); this.name = 'SecretError'; }
}
export class Secret {
  #value;
  constructor(value) { this.#value = Buffer.from(value); }
  expose() { return Buffer.from(this.#value); }
  close() { this.#value.fill(0); }
  toString() { return '[redacted]'; }
  toJSON() { return '[redacted]'; }
  [inspect.custom]() { return 'Secret([redacted])'; }
}
const errors = new Set(['not_initialized', 'not_found', 'store_unavailable', 'invalid_input',
  'invalid_record', 'untrusted_writer', 'not_writer', 'not_granted', 'stale_revision',
  'conflict', 'pending_history', 'storage_failure', 'crypto_failure', 'too_large']);
export class Client {
  constructor(binary = 'hey-boss') { this.binary = binary; }
  #call(request) {
    return new Promise((resolve, reject) => {
      const child = spawn(this.binary, ['secrets', 'rpc'], { stdio: ['pipe', 'pipe', 'ignore'] });
      let chunks = [], size = 0, failed = false;
      const fail = () => {
        failed = true; child.kill('SIGKILL'); clearTimeout(timer);
        for (const chunk of chunks) chunk.fill(0);
        reject(new SecretError('store_unavailable'));
      };
      const timer = setTimeout(fail, 45000);
      child.on('error', fail); child.stdin.on('error', fail);
      child.stdout.on('data', chunk => {
        size += chunk.length;
        if (failed || size > 2 * 1024 * 1024) { chunk.fill(0); fail(); }
        else chunks.push(chunk);
      });
      child.on('close', code => {
        clearTimeout(timer); if (failed) return;
        if (code !== 0) return fail();
        let response;
        const bytes = Buffer.concat(chunks);
        try { response = JSON.parse(bytes.toString('utf8')); }
        catch { return fail(); }
        finally { bytes.fill(0); for (const chunk of chunks) chunk.fill(0); }
        if (response.ok !== true) return reject(new SecretError(errors.has(response.error) ? response.error : 'store_unavailable'));
        resolve(response);
      });
      child.stdin.end(JSON.stringify(request));
    });
  }
  async get(name) {
    const response = await this.#call({ op: 'get', name });
    if (typeof response.data !== 'string' || !response.record) throw new SecretError('invalid_record');
    return { record: response.record, secret: new Secret(Buffer.from(response.data, 'base64')) };
  }
  async put(name, value, { kind = 'value', expected = null } = {}) {
    if (!(value instanceof Uint8Array)) throw new SecretError('invalid_input');
    if (value.length > 1024 * 1024) throw new SecretError('too_large');
    const response = await this.#call({ op: 'put', name, kind, expected, data: Buffer.from(value).toString('base64') });
    return response.record;
  }
}
