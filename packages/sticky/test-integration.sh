#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
cli="$PWD/dist/Sticky.app/Contents/MacOS/sticky"
test -x "$cli"
checks=$(mktemp -d "$PWD/.build/integration.XXXXXX")
trap 'rm -rf "$checks"' EXIT
"$cli" preview Tests/Fixtures/focus.md --output "$checks/focus.png"
"$cli" preview Tests/Fixtures/chart.md --output "$checks/chart.png" --appearance dark
cat Tests/Fixtures/focus.md | "$cli" preview - --output "$checks/stdin.png" --width 320
node --input-type=module - "$checks" <<'JS'
import assert from 'node:assert/strict';
import {readFileSync, writeFileSync} from 'node:fs';
const root = process.argv[2];
for (const [file, width] of [['focus',800],['chart',800],['stdin',640]]) {
  const png = readFileSync(`${root}/${file}.png`);
  assert.equal(png.subarray(1,4).toString(), 'PNG');
  assert.equal(png.readUInt32BE(16), width);
  assert(png.readUInt32BE(20) > 300);
  assert(png.length > 10000, 'Rendered text and diagrams produce a nonempty image');
}
writeFileSync(`${root}/long.md`, '# Long note\n\n' + Array.from({length:50},(_,i) => `${i+1}. A complete line of text.\n`).join(''));
writeFileSync(`${root}/broken.md`, '# Broken diagram\n\n```mermaid\nnot a diagram\n```\n');
writeFileSync(`${root}/missing-image.md`, '# Missing image\n\n![Missing](file:///missing-sticky-test-image.png)\n');
JS
"$cli" preview "$checks/long.md" --output "$checks/long.png"
if "$cli" preview "$checks/broken.md" --output "$checks/broken.png" >"$checks/error.txt" 2>&1; then
  printf 'Malformed diagrams must fail PNG export.\n' >&2; exit 1
fi
test ! -e "$checks/broken.png"
if "$cli" preview "$checks/missing-image.md" --output "$checks/missing-image.png" >"$checks/image-error.txt" 2>&1; then
  printf 'Missing images must fail PNG export.\n' >&2; exit 1
fi
test ! -e "$checks/missing-image.png"
node --input-type=module - "$checks" <<'JS'
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
const root=process.argv[2];
assert(readFileSync(`${root}/long.png`).readUInt32BE(20)>2000, 'Export includes content below the initial viewport');
assert(readFileSync(`${root}/error.txt`,'utf8').includes('Sticky:'), 'Render failure is visible to agents');
console.log('PASS: light/dark PNGs, charts, stdin, width, full-document export, and malformed-diagram errors');
JS
