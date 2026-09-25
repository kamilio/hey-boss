import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
const source = path.resolve(process.argv[2]);
const base = path.dirname(new URL(import.meta.url).pathname);
const dest = path.join(base,'upstream');
const copied = new Map();
function copy(relative) {
  if (copied.has(relative)) return;
  const data = fs.readFileSync(path.join(source,relative));
  copied.set(relative,createHash('sha256').update(data).digest('hex'));
  const output = path.join(dest,relative);
  fs.mkdirSync(path.dirname(output),{recursive:true}); fs.writeFileSync(output,data);
  if (!relative.endsWith('.ts')) return;
  for (const match of data.toString().matchAll(/(?:from\s*|import\s*)["']([^"']+)["']/g)) {
    if (!match[1].startsWith('.')) continue;
    const dependency = path.normalize(path.join(path.dirname(relative),match[1])).replace(/\.js$/,'.ts');
    if (dependency === 'packages/toolcraft-design/src/index.ts') continue;
    copy(dependency);
  }
}
const prefix = 'packages/toolcraft-design/src/terminal-markdown/';
function tests(dir) { return fs.readdirSync(path.join(source,dir),{withFileTypes:true}).flatMap(e => e.isDirectory()?tests(dir+e.name+'/'):e.name.endsWith('.test.ts')?[dir+e.name]:[]); }
const testFiles=tests(prefix).sort();
for(const file of testFiles) copy(file);
copy(prefix+'testing/theme-render-fixture.ts');
copy('packages/frontmatter/src/index.ts');
copy('packages/toolcraft-design/LICENSE');
copy('packages/frontmatter/LICENSE');
fs.writeFileSync(path.join(dest,'packages/toolcraft-design/src/index.ts'),'// Test-only entrypoint for the upstream theme subprocess fixture.\nexport { renderMarkdown } from "./terminal-markdown/index.js";\nexport { resetThemeCache } from "./internal/theme-detect.js";\n');
fs.writeFileSync(path.join(base,'upstream-manifest.json'),JSON.stringify({repository:'https://github.com/poe-platform/poe-code',revision:execFileSync('git',['rev-parse','HEAD'],{cwd:source,encoding:'utf8'}).trim(),testFiles,files:Object.fromEntries([...copied].sort())},null,2)+'\n');
console.log(`Copied ${testFiles.length} complete test files and ${copied.size-testFiles.length} dependencies.`);
