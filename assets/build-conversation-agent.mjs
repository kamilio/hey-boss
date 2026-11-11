// Build Poe Agent from GitHub source; no registry release is required.
import {execFileSync} from 'node:child_process';
import {cp, copyFile, mkdir, readFile, writeFile, realpath} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {homedir} from 'node:os';
import {join, resolve} from 'node:path';
import {pathToFileURL} from 'node:url';

export async function bundle(source, destination) {
  const {build} = await import(pathToFileURL(join(source, 'node_modules/esbuild/lib/main.js')));
  const agent = join(source, 'packages/poe-agent/dist');
  await mkdir(destination, {recursive: true});
  await build({stdin: {contents: [
    `export {agent} from ${JSON.stringify(join(agent, 'agent.js'))};`,
    ...['shell', 'memory', 'skills', 'compaction'].map(name =>
      `export {default as ${name}Plugin} from ${JSON.stringify(join(agent, 'plugins/poe-agent-plugin-' + name + '.js'))};`),
    `export {openaiResponsesPlugin} from ${JSON.stringify(join(agent, 'plugins/poe-agent-plugin-openai-responses.js'))};`,
    `export {parse as parseYaml} from ${JSON.stringify(join(source, 'node_modules/yaml/dist/index.js'))};`,
  ].join('\n'), resolveDir: source}, bundle: true, platform: 'node', format: 'esm', mainFields: ['module', 'main'],
    outfile: join(destination, 'poe-agent.mjs'), external: ['*.node', '*.wasm'],
    banner: {js: 'import {createRequire as __createRequire} from "node:module"; const require = __createRequire(import.meta.url);'}});
  const viz = join(homedir(), '.cache/hey-boss/viz-3.31.0');
  const dist = join(viz, 'node_modules/@viz-js/viz/dist/viz.js');
  if (!existsSync(dist)) execFileSync('npm', ['install', '--prefix', viz, '--ignore-scripts', '--no-audit', '--no-fund', '@viz-js/viz@3.31.0'], {stdio: 'inherit'});
  await copyFile(dist, join(destination, 'viz.mjs'));
  for (const name of ['conversation-dot.mjs', 'conversation-agent.mjs', 'conversation-media.mjs', 'conversation.md', 'conversation-visuals.md']) {
    await copyFile(new URL(name, import.meta.url), join(destination, name));
  }
  await cp(new URL('conversation-tools/', import.meta.url), join(destination, 'conversation-tools'), {recursive: true});
  await copyFile(new URL('mermaid-LICENSE', import.meta.url), join(destination, 'mermaid-LICENSE'));
  await copyFile(join(source, 'LICENSE'), join(destination, 'poe-code-LICENSE'));
}

if (process.argv[1] && import.meta.url === pathToFileURL(await realpath(process.argv[1])).href) {
  const [destination, revision] = process.argv.slice(2);
  if (!destination || !/^[a-f0-9]{40}$/.test(revision ?? '')) throw new Error('Expected output directory and GitHub commit.');
  const source = join(homedir(), '.cache/hey-boss/poe-code-agent');
  await mkdir(join(homedir(), '.cache/hey-boss'), {recursive: true});
  const run = (command, args) => execFileSync(command, args, {cwd: source, stdio: 'inherit'});
  if (!existsSync(source)) execFileSync('git', ['clone', '--depth=1', 'https://github.com/poe-platform/poe-code.git', source], {stdio: 'inherit'});
  const built = await readFile(join(source, '.conversation-built'), 'utf8').catch(() => '');
  if (built !== revision) {
    run('git', ['fetch', '--depth=1', 'origin', revision]);
    run('git', ['checkout', '--detach', revision]);
    run('npm', ['ci', '--no-audit', '--no-fund']);
    run('npm', ['run', 'build:workspaces', '--', '--workspace=@poe-code/poe-agent']);
    await writeFile(join(source, '.conversation-built'), revision);
  }
  // Build once per release; the desktop runs without Cargo or a system Mermaid install.
  const renderer = join(homedir(), '.cache/hey-boss/mermaid');
  execFileSync('cargo', ['install', '--locked', '--version', '=0.3.1', '--no-default-features', '--features', 'cli', '--root', renderer, 'mermaid-rs-renderer'], {stdio: 'inherit'});
  await bundle(source, resolve(destination));
  await copyFile(join(renderer, 'bin/mmdr'), join(destination, 'mmdr'));
  await writeFile(join(destination, 'poe-agent-revision'), revision + '\n');
  // Ship the same standalone Node used for the build, independent of GUI PATH.
  await copyFile(process.execPath, join(destination, 'node'));
}
