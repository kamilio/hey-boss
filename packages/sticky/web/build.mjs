import {build} from 'esbuild';
import {appendFileSync, existsSync, readFileSync} from 'node:fs';
import {dirname, join, resolve} from 'node:path';

const output = '../Sources/Sticky/Resources/diagrams.js';
const result = await build({entryPoints:['diagrams.js'], bundle:true, minify:true, format:'iife',
  target:'safari18', legalComments:'external', outfile:output, metafile:true});
const packages = new Set();
for (const input of Object.keys(result.metafile.inputs)) {
  if (!input.includes('node_modules/')) continue;
  let directory = dirname(resolve(input));
  while (!existsSync(join(directory,'package.json')) || !JSON.parse(readFileSync(join(directory,'package.json'),'utf8')).name) directory = dirname(directory);
  packages.add(directory);
}
for (const directory of [...packages].sort()) {
  const info = JSON.parse(readFileSync(join(directory,'package.json'),'utf8'));
  const license = ['LICENSE','LICENSE.md','LICENSE.txt','license','license.md','license.txt','LICENSE-MIT','COPYING'].map(name=>join(directory,name)).find(existsSync);
  const readme = join(directory,'README.md');
  const text = license ? readFileSync(license,'utf8') : existsSync(readme) ? readFileSync(readme,'utf8').match(/^## License\s*\n([\s\S]*)/m)?.[1] : null;
  if (!text) throw Error('Missing bundled license: '+info.name);
  appendFileSync(output+'.LEGAL.txt', '\n\n'+info.name+' '+info.version+'\n'+text);
}
