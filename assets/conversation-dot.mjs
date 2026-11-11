// Graphviz runs in WASM with no host filesystem, in a time-limited child process.
import {readFileSync} from 'node:fs';
import {instance} from './viz.mjs';
const text = {fontname: 'Helvetica', fontsize: 20};
const viz = await instance();
process.stdout.write(viz.renderString(readFileSync(0, 'utf8'), {
  format: 'svg', engine: 'dot', graphAttributes: text,
  nodeAttributes: {...text, shape: 'box', style: 'rounded,filled', fillcolor: '#edf5ff', color: '#7495b8', fontcolor: '#203c58'},
  edgeAttributes: {...text, color: '#637f96'},
}));
