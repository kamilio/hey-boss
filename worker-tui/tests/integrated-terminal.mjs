// Exercise the installed entry point with a real, isolated queue and PTY.
import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, writeFile, rm } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const { TerminalPilot } = await import(process.env.TERMINAL_PILOT_MODULE ?? "terminal-pilot");
const { renderTerminalPng } = await import(process.env.TERMINAL_PNG_MODULE ?? "terminal-png");
const root = fileURLToPath(new URL("../../", import.meta.url));
const binary = process.env.HEY_BOSS_TUI_TEST_BINARY ?? path.join(root, "target/debug/hey-boss");
const temporary = await mkdtemp(path.join(os.tmpdir(), "hey-boss-integrated-tui-"));
const output = path.join(root, "worker-tui/target/terminal-qa");
const env = { ...process.env, TERM: "xterm-256color", HEY_BOSS_ISSUE_DB: path.join(temporary, "issues.db"),
  HEY_BOSS_CODEX: path.join(root, "tests/fixtures/codex-worker.mjs"), HEY_BOSS_TEST_CLI: binary };
delete env.HEY_BOSS_ISSUE_HOST;
delete env.HEY_BOSS_FLEET_STATE;
delete env.HEY_BOSS_FLEET_MANAGED;
const wrapper = path.join(temporary, "terminal.py");
await mkdir(output, { recursive: true });
await writeFile(wrapper, `import subprocess, sys, termios
before = termios.tcgetattr(0)
code = subprocess.call(sys.argv[1:])
assert termios.tcgetattr(0) == before, "terminal attributes were not restored"
print("TERMINAL_RESTORED", flush=True)
sys.exit(code)
`);
const pilot = await TerminalPilot.launch();
function status() {
  return JSON.parse(execFileSync(binary, ["worker", "--json", "status"], { cwd: temporary, env, encoding: "utf8" }));
}
async function start(args) {
  return pilot.newSession({ command: "python3", args: [wrapper, binary, "worker", ...args],
    cwd: temporary, cols: 120, rows: 36, env });
}
async function quit(session) {
  await session.type("q");
  assert.equal(await session.waitForExit({ timeout: 5000 }), 0);
  assert.match((await session.history()).join("\n"), /TERMINAL_RESTORED/);
}
try {
  await writeFile(path.join(temporary, "mode.txt"), "delay");
  execFileSync(binary, ["issue", "--project", "Worker fixture", "--agent", "human:terminal-tui-qa", "create", "--title", "Fixture dashboard activity", "--body", "Synthetic terminal QA"], { cwd: temporary, env });
  const worker = await start(["run", "--name", "Integrated builder", "--project", "Worker fixture", "--directory", temporary]);
  await worker.waitFor("HEY BOSS", { scope: "screen", timeout: 12000 });
  await worker.waitFor("Integrated builder", { scope: "screen", timeout: 12000 });
  await worker.waitFor("q / Ctrl+C stops this worker", { scope: "screen", timeout: 12000 });
  await worker.waitFor("Fixture dashboard activity", { scope: "screen", timeout: 12000 });
  const live = status().workers.find(w => w.config.name === "Integrated builder");
  assert.ok(live?.pid, "Default TUI did not start the worker");
  await writeFile(path.join(output, "integrated-worker.txt"), (await worker.screen()).text);
  await renderTerminalPng((await worker.screen()).rawLines.join("\n"), { output: path.join(output, "integrated-worker.png") });
  await worker.type("?");
  await worker.waitFor("Keyboard", { scope: "screen", timeout: 1000 });
  await worker.press("Escape");
  await worker.resize(48, 12);
  await worker.waitFor("Active agents", { scope: "screen", timeout: 1000 });
  await worker.resize(120, 36);
  // Status is a dashboard too, but quitting it must leave the worker running.
  const dashboard = await start(["status"]);
  await dashboard.waitFor("Integrated builder", { scope: "screen", timeout: 12000 });
  await quit(dashboard);
  assert.ok(status().workers.find(w => w.id === live.id)?.pid);
  const plain = execFileSync(binary, ["worker", "status"], { cwd: temporary, env, encoding: "utf8" });
  assert.doesNotMatch(plain, /\x1b\[/);
  assert.ok(status().ok, "JSON status must remain machine-readable");
  await quit(worker);
  assert.equal(status().workers.find(w => w.id === live.id)?.pid, null, "Quit leaked the owned worker");
  const finished = JSON.parse(execFileSync(binary, ["worker", "--json", "--id", live.id, "--history", "20", "status"], { cwd: temporary, env, encoding: "utf8" }));
  assert.equal(finished.active, 0, "Quit leaked an owned Codex session");
  assert.ok(finished.runs.some(r => r.state === "cancelled"), `Owned session was not finalized on quit: ${JSON.stringify(finished.runs)}`);
  console.log("Integrated worker/status terminal-pilot walkthrough passed.");
} finally {
  await pilot.close();
  await rm(temporary, { recursive: true, force: true });
}
