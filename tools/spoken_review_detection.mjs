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
  return readTurns(Buffer.concat(parts), []);
}
function readTurns(input, args) {
  const result = spawnSync(cli, ['review-audio', ...args], {input, maxBuffer: 8 * 1024 * 1024});
  assert.equal(result.status, 0, String(result.stderr));
  const turns = [];
  let turn, bookmarked = false, continued = false;
  for (let i = 0; i < result.stdout.length;) {
    const count = result.stdout.readUInt16LE(i); i += 2;
    if (count === 2) {
      if (turn) {
        assert(args.includes('--conversation'), 'review comments only bookmark the initial onset');
        continued = true;
      } else bookmarked = true;
      continue;
    }
    if (count === 0) { assert(bookmarked && !turn); turn = []; bookmarked = false; continue; }
    if (count === 1) {
      if (args.includes('--conversation')) assert(continued, 'sustained voice in the chat fixture must confirm continued speech');
      turns.push(Buffer.concat(turn)); turn = null; continued = false; continue;
    }
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
// Frequent key strikes can keep a conventional voice gate open for several seconds.
const typing = signal(64000, i => (i % 2400 < 160 ? Math.exp(-(i % 2400) / 45) * noise.readInt16LE((i % 48000) * 2) * 18 : 0));
const chat = (...parts) => readTurns(Buffer.concat([silence(1000), ...parts, silence(3500)]), ['--conversation']);
const ping = spawnSync('ffmpeg', ['-loglevel', 'error', '-i', '/System/Library/Sounds/Ping.aiff', '-ar', '16000', '-ac', '1', '-f', 's16le', '-']);
assert.equal(ping.status, 0, String(ping.stderr));
assert.equal(chat(ping.stdout).length, 0, 'a notification must not open a chat turn');
assert.equal(chat(typing).length, 0, 'typing alone must not open a chat turn');
for (const speech of [voice, quiet]) {
  const mixed = signal(speech.length / 2, i => Math.max(-32768, Math.min(32767, speech.readInt16LE(i * 2) + typing.readInt16LE((i * 2) % typing.length))));
  const turns = chat(typing, mixed, typing);
  assert.equal(turns.length, 1, 'speech amid typing opens one chat turn');
  assert(turns[0].length < mixed.length + 32000 * 3.5, 'continued typing must not hold the turn open');
}
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

// The webcam hears a delayed, attenuated copy of the speakers. The second
// fixture is a different speaker talking over that same playback.
const other = readFileSync(process.argv[4]);
for (const interrupt of [false, true]) {
  const seconds = 8, stereo = Buffer.alloc(seconds * 16000 * 4);
  function rendered(i) {
    return i < 16000 || i >= 96000 ? 0 : voice.readInt16LE(2 * ((i - 16000) % (voice.length / 2)));
  }
  for (let i = 0; i < seconds * 16000; i++) {
    const j = i - 40000;
    const near = interrupt && j >= 0 && j < other.length / 2 ? other.readInt16LE(j * 2) * 0.15 : 0;
    stereo.writeInt16LE(Math.round(rendered(i - 1440) * 0.08 + near), i * 4);
    stereo.writeInt16LE(rendered(i), i * 4 + 2);
  }
  assert.equal(readTurns(stereo, ['--duplex']).length, Number(interrupt));
}
console.log('PASS speaker echo alone stays silent; overlapping speech opens one comment');
