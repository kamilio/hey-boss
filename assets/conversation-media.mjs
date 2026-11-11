import {execFile} from 'node:child_process';
import {open} from 'node:fs/promises';
import {homedir} from 'node:os';
import {basename, extname, resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {renderGraph, graphvizLimits} from './poe-agent.mjs';

const limit = 12 * 1024 * 1024;
const string = {type: 'string'};
export const mediaSchema = {type: 'object', properties: {
  title: string, file: string, url: string, svg: string, graphviz: string, mermaid: string,
  image: {type: 'object', properties: {type: string, mimeType: string, data: string,
    image_url: {anyOf: [string, {type: 'object', properties: {url: string}, required: ['url'], additionalProperties: false}]}}, additionalProperties: false},
}, additionalProperties: false};

function checkSize(size, max = limit) {
  if (size > max) throw new Error('Media exceeds the size limit.');
}

async function readBytes(stream) {
  const chunks = []; let size = 0;
  for await (const chunk of stream) {
    size += chunk.length; checkSize(size); chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

export async function prepareMedia({title, ...sources}, cwd, signal) {
  const entries = Object.entries(sources).filter(([, value]) => value != null);
  if (entries.length !== 1) throw new Error('Provide one file, URL, SVG, Graphviz, Mermaid, or image part.');
  let [[format, source]] = entries;
  let bytes, name;
  if (format === 'image') {
    if (source.type === 'image' && /^image\//.test(source.mimeType) && typeof source.data === 'string') {
      checkSize(source.data.length, Math.ceil(limit / 3) * 4);
      bytes = Buffer.from(source.data, 'base64');
      if (source.mimeType === 'image/svg+xml') format = 'svg';
    } else {
      source = source.image_url?.url ?? source.image_url;
      format = 'url';
    }
  }
  if (!bytes && (typeof source !== 'string' || !source.trim())) throw new Error('Missing media source.');
  let extension;
  if (format === 'file') {
    const path = source.startsWith('~/') ? resolve(homedir(), source.slice(2)) : resolve(cwd, source);
    const file = await open(path, 'r');
    try {
      const stat = await file.stat();
      if (!stat.isFile()) throw new Error('Expected an image or diagram file.');
      checkSize(stat.size);
      bytes = await readBytes(file.createReadStream({signal}));
    } finally { await file.close(); }
    name = basename(path); extension = extname(path);
  } else if (format === 'url') {
    if (!/^(https?:\/\/|data:image\/)/i.test(source)) throw new Error('Expected an HTTP(S) image URL or image data URL.');
    if (source.startsWith('data:')) checkSize(source.length, limit * 1.4);
    const response = await fetch(source, {signal: AbortSignal.any([signal, AbortSignal.timeout(20000)])});
    if (!response.ok) throw new Error('Image request failed: HTTP ' + response.status);
    bytes = await readBytes(response.body);
    if (response.headers.get('content-type')?.startsWith('image/svg+xml')) format = 'svg';
    if (!source.startsWith('data:')) extension = extname(new URL(response.url).pathname);
  }
  format = ({'.dot': 'graphviz', '.gv': 'graphviz', '.mmd': 'mermaid', '.mermaid': 'mermaid', '.svg': 'svg'})[extension?.toLowerCase()] ?? format;
  bytes ??= Buffer.from(source);
  checkSize(bytes.length);
  if (format === 'graphviz' || format === 'mermaid') checkSize(bytes.length, 1024 * 1024);
  if (format === 'graphviz') {
    bytes = Buffer.from(await renderGraph(bytes.toString(), 'svg', 'dot', {
      node: {shape: 'box', style: 'rounded,filled', fillcolor: '#edf5ff', color: '#7495b8', fontcolor: '#203c58', fontname: 'Helvetica'},
      edge: {color: '#637f96'},
    }, graphvizLimits({}), signal));
  } else if (format === 'mermaid') {
    const input = bytes;
    bytes = await new Promise((ok, fail) => {
      const child = execFile(fileURLToPath(new URL('./mmdr', import.meta.url)), ['-i', '-', '-t', 'modern'],
        {signal, timeout: 10000, maxBuffer: limit, encoding: 'buffer'}, (error, stdout, stderr) =>
          error ? fail(new Error(stderr.toString().trim() || error.message)) : ok(stdout));
      child.stdin.on('error', error => { if (error.code !== 'EPIPE') fail(error); });
      child.stdin.end(input);
    });
  }
  checkSize(bytes.length);
  const kind = ({svg: 'SVG', mermaid: 'Mermaid', graphviz: 'Graphviz'})[format] ?? 'Image';
  return {kind, title: title?.trim() || name || (kind === 'Image' ? 'Image' : 'Diagram'), data: bytes.toString('base64')};
}
