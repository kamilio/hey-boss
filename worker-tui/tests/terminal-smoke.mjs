// Real PTY verification. The queue is synthetic; no real workers are controlled.
import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, writeFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const { TerminalPilot } = await import(process.env.TERMINAL_PILOT_MODULE ?? "terminal-pilot");
const { renderTerminalPng } = await import(process.env.TERMINAL_PNG_MODULE ?? "terminal-png");
const root = fileURLToPath(new URL("../", import.meta.url));
const output = path.join(root, "target/terminal-qa");
const temporary = await mkdtemp(path.join(os.tmpdir(), "hey-boss-tui-qa-"));
const fixture = path.join(temporary, "queue.py");
const statePath = path.join(temporary, "state.json");
const delayPath = path.join(temporary, "slow");
const failPath = path.join(temporary, "offline");
const removedPath = path.join(temporary, "removed");
const wrapper = path.join(temporary, "terminal.py");
const binary = path.join(root, "target/debug/hey-boss-worker-tui");
const python = process.env.PYTHON ?? "python3";
const pilot = await TerminalPilot.launch();
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
await mkdir(output, { recursive: true });
await writeFile(statePath, "{}");
await writeFile(fixture, `#!/usr/bin/env python3
import json, pathlib, sys, time
root = pathlib.Path(__file__).parent
args = sys.argv[1:]
state = json.loads((root / "state.json").read_text())
if "pause" in args or "stop" in args:
    action = "stop" if "stop" in args else "pause"
    state[args[args.index(action)+1]] = action
    pending = root / "state.json.tmp"
    pending.write_text(json.dumps(state))
    pending.replace(root / "state.json")
    print(json.dumps({"ok": True}))
    sys.exit(0)
if (root / "offline").exists():
    print("synthetic queue offline", file=sys.stderr)
    sys.exit(1)
if (root / "slow").exists(): time.sleep(4)
selected = args[args.index("--id")+1] if "--id" in args else "alpha"
if (root / "removed").exists() and selected == "beta":
    print("Worker was not found", file=sys.stderr)
    sys.exit(1)
names = [("alpha", "Builder"), ("beta", "Reviewer")]
if (root / "removed").exists(): names = names[:1]
workers = [{"id": key, "pid": None if state.get(key) == "stop" else 42,
    "active": 1 if key == "alpha" else 0,
    "config": {"name": name, "concurrency": 2, "enabled": key not in state}}
    for key, name in names]
now = int(time.time()*1000)
print(json.dumps({"ok": True, "workers": workers, "worker_id": selected,
    "config": {"concurrency": 2, "tags": ["ready"]},
    "active": 1, "free": 1, "eligible": 3,
    "store": {"host": "synthetic", "database": "/tmp/test-queue.db"},
    "runs": [
        {"id": selected+"-live", "project_name": "demo", "number": 6,
         "title": "Build dashboard" if selected == "alpha" else "Review dashboard",
         "state": "running", "finished_at": None, "started_at": now-62000,
         "session_id": "synthetic-session", "last_event": "Running checks",
         "events": [{"text": "Synthetic activity " + str(n)} for n in range(3)]},
        {"id": selected+"-done", "project_name": "demo", "number": 5,
         "title": "Previous attempt", "state": "completed", "finished_at": now,
         "started_at": now-60000}
    ]}))
`, { mode: 0o700 });
// Observe terminal attributes before/after the dashboard, not just its exit code.
await writeFile(wrapper, `import pathlib, subprocess, sys, termios
before = termios.tcgetattr(0)
child = subprocess.Popen(sys.argv[1:])
pathlib.Path(__file__).with_suffix(".pid").write_text(str(child.pid))
code = child.wait()
assert termios.tcgetattr(0) == before, "terminal attributes were not restored"
print("TERMINAL_RESTORED", flush=True)
sys.exit(code)
`);

async function start() {
  return pilot.newSession({ command: python, args: [wrapper, binary, "--binary", fixture],
    cwd: root, cols: 120, rows: 36, env: { ...process.env, TERM: "xterm-256color" } });
}
async function wait(session, pattern) {
  await session.waitFor(pattern, { scope: "screen", timeout: 12000 });
}
// Automatic refresh disables controls briefly. A rendered Live footer can become
// stale between observation and input; retry opening the modal, never confirming it.
async function openConfirmation(session, key, title) {
  const deadline = Date.now() + 12000;
  while (Date.now() < deadline) {
    await session.type(key);
    try {
      await session.waitFor(title, { scope: "screen", timeout: 500 });
      return;
    } catch (error) {
      if (!error.message.includes("Timed out waiting for pattern")) throw error;
    }
  }
  throw new Error(`Control confirmation did not open: ${title}`);
}
async function capture(session, name) {
  await session.waitForQuiet(100);
  const screen = await session.screen();
  await writeFile(path.join(output, `${name}.txt`), screen.text);
  await renderTerminalPng(screen.rawLines.join("\n"), { output: path.join(output, `${name}.png`) });
}
async function ready(session) {
  await wait(session, "Live · refresh");
}
async function exit(session, key = "q") {
  if (key === "Control+c") { await session.press(key); } else { await session.type(key); }
  assert.equal(await session.waitForExit({ timeout: 3000 }), 0);
  assert.match((await session.history()).join("\n"), /TERMINAL_RESTORED/);
}
async function state() { return JSON.parse(await readFile(statePath, "utf8")); }
async function waitState(expected) {
  const deadline = Date.now() + 3000;
  while (JSON.stringify(await state()) !== JSON.stringify(expected) && Date.now() < deadline) {
    await delay(25);
  }
  assert.deepEqual(await state(), expected);
}

try {
  const session = await start();
  await wait(session, "Build dashboard");
  assert.ok(!(await session.screen()).contains("Previous attempt"), "Default must show only active work");
  await capture(session, "wide");
  await session.type("h");
  await wait(session, "Previous attempt");
  assert.ok(!(await session.screen()).contains("Build dashboard"), "History must be separate from active work");
  await session.press("ArrowDown");
  await session.press("PageDown");
  await session.type("?");
  await wait(session, "Keyboard");
  await session.press("Escape");
  await session.resize(48, 12);
  await wait(session, "Completed attempts");
  assert.ok((await session.screen()).contains("Previous attempt"), "Compact layout hides the selected session");
  await capture(session, "compact");
  await session.resize(30, 8);
  await wait(session, "Resize to at least");
  await session.type("sp");
  assert.deepEqual(await state(), {}, "Small-screen keys must not control workers");
  await session.resize(120, 36);
  await session.type("h");
  await wait(session, "Build dashboard");
  await ready(session);
  await writeFile(delayPath, "");
  await session.type("r");
  await wait(session, "Refreshing");
  const time = Date.now();
  await session.type("?");
  await wait(session, "Keyboard");
  assert.ok(Date.now() - time < 1000, "Help blocked on the queue read");
  await session.press("Escape");
  await rm(delayPath);
  await ready(session);
  assert.ok(!(await session.screen()).contains("Reviewer"), "Other workers must stay out of the current worker dashboard");
  // Errors retain the good snapshot and recover without restarting the UI.
  await writeFile(failPath, "");
  await session.type("r");
  await wait(session, "synthetic queue offline");
  assert.ok((await session.screen()).contains("Build dashboard"));
  await rm(failPath);
  await session.type("r");
  await ready(session);
  await session.send("\x1b[200~sp\x1b[201~"); // pasted controls are not key presses
  await delay(100);
  assert.deepEqual(await state(), {});
  await openConfirmation(session, "s", "Stop worker?");
  await capture(session, "confirmation");
  await session.press("Escape");
  assert.deepEqual(await state(), {}, "Cancelled confirmation changed the queue");
  await ready(session);
  await openConfirmation(session, "p", "Pause worker?");
  await session.press("Enter");
  await waitState({ alpha: "pause" });
  await wait(session, "Finishing work before pause");
  await ready(session);
  await openConfirmation(session, "s", "Stop worker?");
  await session.press("Enter");
  await waitState({ alpha: "stop" });
  await wait(session, "stopped");
  await ready(session);
  assert.deepEqual(await state(), { alpha: "stop" });
  await exit(session);
  // Quit during a blocked request and external signals both restore the terminal.
  await writeFile(delayPath, "");
  const blocked = await start();
  await wait(blocked, "Connecting to queue");
  await exit(blocked, "Control+c");
  await rm(delayPath);
  const signalled = await start();
  await wait(signalled, "Build dashboard");
  process.kill(Number(await readFile(wrapper.replace(".py", ".pid"), "utf8")), "SIGTERM");
  assert.equal(await signalled.waitForExit({ timeout: 3000 }), 0);
  assert.match((await signalled.history()).join("\n"), /TERMINAL_RESTORED/);
  console.log(`Terminal-pilot walkthrough passed. Screenshots: ${output}`);
} finally {
  await pilot.close();
  await rm(temporary, { recursive: true, force: true });
}
