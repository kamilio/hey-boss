// Real PTY design review, using only synthetic data and Node fixtures.
import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const { TerminalPilot } = await import(process.env.TERMINAL_PILOT_MODULE ?? "terminal-pilot");
const { renderTerminalPng } = await import(process.env.TERMINAL_PNG_MODULE ?? "terminal-png");
const root = fileURLToPath(new URL("../", import.meta.url));
const temporary = await mkdtemp(path.join(os.tmpdir(), "hey-boss-activity-qa-"));
const output = path.join(root, "target/activity-qa");
const fixture = path.join(temporary, "queue.mjs");
const binary = path.join(root, "target/debug/hey-boss-worker-tui");
const pilot = await TerminalPilot.launch();
await mkdir(output, { recursive: true });
const now = Date.now();
const events = [
  "The blue theme is in place. Checking keyboard navigation and narrow terminals.\nThe selected agent stays visible while reading its activity.",
  "Command: cargo test --locked --manifest-path worker-tui/Cargo.toml\nAll dashboard regression checks passed.",
  "Preserved multiline messages and removed duplicate latest updates.\nOlder updates remain available with Page Down.",
  "Reviewing the app palette and activity hierarchy.",
  ...Array.from({ length: 8 }, (_, n) => `Earlier update ${n + 1}: checking layout, responsiveness, and retained session context.`),
].map((text, n) => ({ at: now - (n + 1) * 30_000, text }));
const snapshot = {
  ok: true, worker_id: "design", fleet: { supervisor_connection: { state: "connected" } },
  workers: [{ id: "design", pid: 42, active: 2,
    config: { name: "MacBook builder", enabled: true, concurrency: 3,
      projects: ["named:hey-boss"], directory: "/workspace/hey-boss" } }],
  runs: [
    { id: "design-live", project_name: "hey-boss", number: 86, title: "Improve terminal dashboard",
      state: "running", started_at: now - 387_000, finished_at: null,
      last_event: events[0].text, events, goal: { status: "active", objective: "Improve TUI" } },
    { id: "second", project_name: "hey-boss", number: 91, title: "Verify companion connectivity",
      state: "running", started_at: now - 94_000, finished_at: null, events: [] },
    { id: "failed", project_name: "hey-boss", number: 85, title: "Upgrade connected device",
      state: "failed", started_at: now - 900_000, finished_at: now - 450_000,
      summary: "The device disconnected during verification.\nReconnect the device, then retry the upgrade.",
      events: [{ at: now - 450_000, text: "SSH connection closed before verification finished." }] },
  ],
};
await writeFile(fixture, `#!/usr/bin/env node\nimport { existsSync } from 'node:fs';\nif (existsSync(${JSON.stringify(path.join(temporary, "offline"))})) { console.error('Synthetic queue offline'); process.exit(1); }\nconsole.log(${JSON.stringify(JSON.stringify(snapshot))});\n`, { mode: 0o700 });
const wrapper = path.join(temporary, "terminal.sh");
await writeFile(wrapper, '#!/bin/sh\nbefore=$(stty -g)\n"$@" <&0 &\nchild=$!\ntrap \'kill -TERM "$child" 2>/dev/null; wait "$child"; stty "$before"; exit 143\' TERM INT HUP\nwait "$child"\ncode=$?\n[ "$(stty -g)" = "$before" ] || exit 99\nprintf "TERMINAL_RESTORED\\n"\nexit "$code"\n', { mode: 0o700 });
const wait = (session, pattern) => session.waitFor(pattern, { scope: "screen", timeout: 12000 });
async function capture(session, name) {
  await session.waitForQuiet(100);
  const screen = await session.screen();
  await writeFile(path.join(output, `${name}.txt`), screen.text);
  await renderTerminalPng(screen.rawLines.join("\n"), { output: path.join(output, `${name}.png`) });
  return screen;
}
try {
  const session = await pilot.newSession({ command: wrapper, args: [binary, "--binary", fixture],
    cwd: root, cols: 120, rows: 36, env: { ...process.env, TERM: "xterm-256color", COLORTERM: "truecolor", NO_COLOR: "" } });
  await wait(session, "Improve terminal dashboard");
  const wide = await capture(session, "01-wide");
  assert.match(wide.rawLines.join("\n"), /38;2;161;175;255/, "Review must capture actual blue terminal colors");
  await session.press("PageDown");
  await wait(session, "Older");
  await capture(session, "02-older");
  await session.press("ArrowDown");
  await wait(session, "Waiting for agent activity");
  await wait(session, "Latest");
  const waiting = await capture(session, "03-waiting");
  assert.ok(!waiting.contains("Older"), "Selection must reset log scroll");
  await session.type("h");
  await wait(session, "Reconnect the device");
  await capture(session, "04-failure-history");
  await session.type("h");
  await wait(session, "The blue theme is in place");
  for (const [cols, rows, name] of [[80, 24, "05-standard"], [64, 18, "06-narrow"], [48, 12, "07-minimum"], [30, 8, "08-too-small"]]) {
    await session.resize(cols, rows);
    // The old session heading can survive PTY resize before the next draw.
    // Wait for the footer at its new position, rather than capturing that stale screen.
    await wait(session, cols < 48 ? "Resize to at least" : "↑↓ session  h history  ? help  q quit");
    await capture(session, name);
    if (cols >= 64) assert.ok((await session.screen()).contains("The blue theme is in place"), "Short terminals must show activity, not only metadata");
    if (cols === 48) {
      await session.type("?");
      await wait(session, "Close help");
      await capture(session, "07-minimum-help");
      await session.press("Escape");
      await wait(session, "Supervisor: connected");
      await wait(session, "#86 running");
      await session.waitFor(/^(?![\s\S]*Keyboard)[\s\S]*$/, { scope: "screen", timeout: 2000 });
    }
  }
  await session.resize(120, 36);
  await wait(session, "The blue theme is in place");
  await session.type("?");
  await wait(session, "Close help");
  await capture(session, "09-help");
  await session.press("Escape");
  await writeFile(path.join(temporary, "offline"), "");
  await session.type("r");
  await wait(session, "Synthetic queue offline");
  assert.ok((await session.screen()).contains("Improve terminal dashboard"));
  await capture(session, "10-disconnected");
  await rm(path.join(temporary, "offline"));
  await session.type("r");
  await wait(session, "Supervisor: connected");
  await session.type("q");
  assert.equal(await session.waitForExit({ timeout: 5000 }), 0);
  assert.match((await session.history()).join("\n"), /TERMINAL_RESTORED/);
  console.log(`Activity terminal-pilot review passed: ${output}`);
} catch (error) {
  for (const session of pilot.sessions()) {
    if (session.exitCode === null) await capture(session, "failure");
  }
  throw error;
} finally {
  // Quit the application before closing its shell's PTY, including on assertions.
  // Closing only the wrapper can otherwise leave an orphaned dashboard.
  for (const session of pilot.sessions()) {
    if (session.exitCode === null) {
      await session.type("q").catch(() => {});
      await session.waitForExit({ timeout: 3000 }).catch(() => {});
    }
  }
  await pilot.close();
  await rm(temporary, { recursive: true, force: true });
}
