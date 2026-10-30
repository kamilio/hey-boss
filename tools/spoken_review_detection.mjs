// Feed the production detector known audio. The 400 ms pre-roll lets the
// returned PCM identify the exact frame where the detector interrupted.
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import {readFileSync} from 'node:fs';
const [cli, file] = process.argv.slice(2), voice = readFileSync(file);
const silence = ms => Buffer.alloc(ms * 32);
function signal(samples, fn) {
  const bytes = Buffer.alloc(samples * 2);
  for (let i = 0; i < samples; ++i) bytes.writeInt16LE(Math.round(fn(i)), i * 2);
  return bytes;
}
function detect(...parts) {
  const input = Buffer.concat(parts), result = spawnSync(cli, ['review-audio'], {input, maxBuffer: 8 * 1024 * 1024});
  assert.equal(result.status, 0, String(result.stderr));
  const turns = [];
  let turn, bookmarked = false;
  for (let i = 0; i < result.stdout.length;) {
    const count = result.stdout.readUInt16LE(i); i += 2;
    if (count === 2) { bookmarked = true; continue; }
    if (count === 0) { assert(bookmarked && !turn); turn = []; bookmarked = false; continue; }
    if (count === 1) { turns.push(Buffer.concat(turn)); turn = null; continue; }
    assert(turn && count === 640 && result.stdout.length - i >= count);
    turn.push(result.stdout.subarray(i, i + count)); i += count;
  }
  assert(!turn, 'unterminated comment');
  return turns;
}
let seed = 12345;
const noise = signal(48000, () => ((seed = seed * 16807 % 2147483647) / 2147483647 * 2 - 1) * 1000);
const click = signal(16000, i => i > 3000 && i < 3070 ? 20000 * Math.sin(i) : 0);
assert.equal(detect(silence(1000), noise, silence(1000), click, silence(1500)).length, 0);
const quiet = signal(voice.length / 2, i => voice.readInt16LE(i * 2) / 10);
for (const sound of [voice, quiet]) {
  const input = Buffer.concat([silence(1000), sound, silence(1500)]);
  const turns = detect(input);
  assert.equal(turns.length, 1);
  const start = input.indexOf(turns[0]) / 32;
  assert(start >= 600 && start < 1000, 'pre-roll includes the first word');
  const latency = start + 400 - 1000;
  assert(latency >= 100 && latency <= 320, 'prompt speech onset');
  const tail = start + turns[0].length / 32 - (1000 + sound.length / 32);
  assert(tail >= 650 && tail <= 1150, 'deliberate end pause');
  console.log('PASS interruption at %d ms, endpoint %d ms after fixture', latency, Math.round(tail));
}
for (const [gap, count] of [[300, 1], [500, 1], [1300, 2]]) {
  assert.equal(detect(silence(1000), voice, silence(gap), voice, silence(1500)).length, count);
}
console.log('PASS background noise, click, quiet speech, thinking pauses and separate comments');
