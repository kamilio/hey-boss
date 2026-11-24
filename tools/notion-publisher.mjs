#!/usr/bin/env node
// Adapter from Pages' publisher contract to Wire's Notion Markdown sync.
import {execFileSync} from 'node:child_process';
import {mkdtempSync, readFileSync, writeFileSync, rmSync} from 'node:fs';
import {dirname, join, resolve} from 'node:path';
import {createHash} from 'node:crypto';

const [operation, source, title, id, version] = process.argv.slice(2);
const hash = text => createHash('sha256').update(text).digest('hex');
const notionURL = value => {
  const url = new URL(value);
  if (url.protocol !== 'https:' || !['notion.so','www.notion.so'].includes(url.hostname)) throw Error('Expected a Notion page URL');
  return url.href;
};
let stage;
try {
  if (!['create','read','update'].includes(operation)) throw Error('Expected create, read, or update');
  // Keep Wire's registry and its base revision together, isolated from the user's workspace.
  stage = mkdtempSync(join(operation === 'read' ? process.cwd() : dirname(resolve(source)), '.notion-'));
  const wire = (...args) => JSON.parse(execFileSync('wire', [...args, '--output', 'json'], {cwd:stage, encoding:'utf8', maxBuffer:8*1024*1024, stdio:['ignore','pipe','pipe']}));
  wire('init', '--backend', 'files');
  let current;
  if (operation !== 'create') {
    notionURL(id);
    current = wire('attach', id);
    if (operation === 'update' && hash(current.markdown) !== version) throw Error('The Notion page changed since the last upload. Review its edits before linking it again.');
  }
  let result = current;
  if (operation !== 'read') {
    const path = current?.path || join(stage, 'document.md');
    if (!resolve(path).startsWith(stage + '/')) throw Error('Wire returned a file outside its publishing workspace');
    let body = readFileSync(source, 'utf8');
    // The artifact title is authoritative; avoid duplicating its matching first heading.
    if (body.split('\n')[0].replace(/^#\s+/, '').trim() === title.trim()) body = body.includes('\n') ? body.slice(body.indexOf('\n') + 1) : '';
    writeFileSync(path, `# ${title}\n\n${body}`);
    result = wire('sync', path);
  }
  const url = notionURL(operation === 'create' ? result.summary.remote : id);
  process.stdout.write(JSON.stringify({id:url, url, version:hash(result.markdown)}));
} catch (error) {
  let message = error.stderr?.toString().trim() || error.message;
  try { message = JSON.parse(error.stdout).error?.message || message; } catch {}
  process.stderr.write(message + '\n');
  process.exitCode = 1;
} finally {
  if (stage) rmSync(stage, {recursive:true, force:true});
}
