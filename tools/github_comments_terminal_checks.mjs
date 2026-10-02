// Real PTY, synthetic GitHub executable: no network or live comments.
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, writeFile, readFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {TerminalPilot} from '../worker-tui/node_modules/terminal-pilot/dist/index.js';
import {renderTerminalPng} from '../worker-tui/node_modules/terminal-png/dist/index.js';

const binary = resolve(process.argv[2] || 'target/debug/hey-gh');
const output = process.argv[3] && resolve(process.argv[3]);
const root = await mkdtemp(join(tmpdir(), 'github-comment-terminal-'));
let pilot;
try {
  if (output) await mkdir(output, {recursive: true});
  await writeFile(join(root, 'gh'), '#!/bin/sh\ncat > "$FIXTURE/body"\nif [ "$GH_EXIT" = 1 ]; then echo "GitHub unavailable" >&2; exit 1; fi\nprintf "%s\\n" "https://github.com/example/repo/pull/7#issuecomment-1"\n', {mode: 0o700});
  pilot = await TerminalPilot.launch();
  let checked = 0;
  for (const width of [48, 80, 120]) {
    for (const [name, args, exit, expected, upstreamExit] of [
      ['help', ['pr', 'comment', '--help'], 0, 'Do not sound like a robot.', '0'],
      ['rejection', ['pr', 'comment', '7', '--body', 'x'.repeat(301)], 1, '300 characters.', '0'],
      ['success', ['pr', 'comment', '7', '--body', 'Fixed the retry. Tests pass.'], 0, '#issuecomment-1', '0'],
      ['failure', ['pr', 'comment', '7', '--body', 'Fixed the retry.'], 1, 'not retried.', '1'],
    ]) {
      const session = await pilot.newSession({command: binary, args, cwd: root, cols: width, rows: 40,
        env: {...process.env, PATH: root + ':/usr/bin:/bin', FIXTURE: root, GH_EXIT: upstreamExit, TERM: 'xterm-256color'}});
      assert.equal(await session.waitForExit({timeout: 10000}), exit);
      const screen = await session.screen();
      // Wrapped text must remain complete and readable at all widths.
      const text = screen.text.replace(/\s+/g, '');
      assert(text.includes(expected.replace(/\s+/g, '')), `${name} at ${width}: ${screen.text}`);
      if (name === 'success') assert.equal(await readFile(join(root, 'body'), 'utf8'), 'Fixed the retry. Tests pass.');
      if (output) await renderTerminalPng(screen.rawLines.join('\n'), {output: join(output, `${name}-${width}.png`)});
      await session.close();
      checked++;
    }
  }
  console.log(`${checked} terminal checks passed at 48, 80, and 120 columns.`);
} finally {
  if (pilot) await pilot.close();
  await rm(root, {recursive: true, force: true});
}
