#!/usr/bin/env bun
// Separate Poe collector; run from the Poe checkout so Bun loads its credentials.
import {
  mkdir,
  open,
  readFile,
  rename,
  unlink,
  writeFile,
} from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { mapEvent } from "./poe-errors-core";
const repo = resolve(process.env.POE_REPO ?? process.cwd());
const binary = process.env.HEY_BOSS_BIN ?? "hey-boss";
const project = "named:poe2-legacy";
async function cli(args: string[], input?: unknown) {
  const child = Bun.spawn([binary, ...args], {
    cwd: repo,
    stdin: input === undefined ? "ignore" : "pipe",
    stdout: "pipe",
    stderr: "pipe",
  });
  if (input !== undefined) {
    child.stdin.write(JSON.stringify(input));
    child.stdin.end();
  }
  const [text, error, status] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (status !== 0) throw Error(`hey-boss ${args[0]} failed: ${error.trim()}`);
  const result = JSON.parse(text);
  if (!result.ok)
    throw Error(result.error?.message ?? "Inbox operation failed");
  return result;
}
const who = await cli(["issue", "whoami", "--agent", "watcher:poe-errors", "--json"]);
if (!who.store?.database) throw Error("Collector needs a local issue store");
const directory = join(dirname(who.store.database), "poe-error-collector");
await mkdir(directory, { recursive: true, mode: 0o700 });
const checkpointPath = join(directory, "checkpoint.json"),
  lockPath = join(directory, "collector.lock");
async function lock() {
  try {
    const fd = await open(lockPath, "wx", 0o600);
    await fd.writeFile(String(process.pid));
    await fd.close();
  } catch (e: any) {
    if (e.code !== "EEXIST") throw e;
    const pid = Number(await readFile(lockPath, "utf8"));
    if (!Number.isInteger(pid) || pid <= 0)
      throw Error(
        "Collector lock is incomplete; check for another running collector",
      );
    try {
      process.kill(pid, 0);
    } catch (error: any) {
      if (error.code === "ESRCH") {
        await unlink(lockPath);
        return lock();
      }
      throw error;
    }
    throw Error(`Collector already running (PID ${pid})`);
  }
}

const api = await import(`${repo}/scripts/cloudflare-worker-logs.ts`);
// Legacy preview is in the development account, independent of production defaults.
const accountId = "fdb283a7279a7b4d1f3577dbb2089ff2";
const { token } = api.resolveWorkerLogsToken({ env: process.env });
await lock();
const workers = ["poe-tiles-poe--legacy", "poe-tiles-store-poe--legacy"];
let stopped = false;
process.on("SIGTERM", () => {
  stopped = true;
});
process.on("SIGINT", () => {
  stopped = true;
});
async function collect(
  worker: string,
  from: number,
  to: number,
): Promise<number> {
  const body = api.buildTelemetryQuery({
    worker,
    level: "error",
    timeframe: { from, to },
    limit: 1000,
  });
  const response = await api.queryWorkerLogs({ accountId, token, body });
  const rows = response.json?.result?.events?.events;
  if (!Array.isArray(rows)) throw Error(`Missing event result for ${worker}`);
  if (rows.length >= 1000) {
    if (to - from <= 1000)
      throw Error(
        "One-second log window reached the cap; checkpoint retained for retry",
      );
    const mid = Math.floor((from + to) / 2);
    return (
      (await collect(worker, from, mid)) + (await collect(worker, mid, to))
    );
  }
  const events = rows.map((row: any) => mapEvent(row, worker));
  let accepted = 0;
  for (let offset = 0; offset < events.length; offset += 100) {
    const result = await cli(
      ["errors", "--project", project, "ingest"],
      events.slice(offset, offset + 100),
    );
    accepted += result.accepted;
  }
  return accepted;
}
async function poll() {
  const settings = await cli(["errors", "--project", project, "list"]);
  if (!settings.enabled) {
    console.log("Poe error collection is disabled; no upstream query made.");
    return;
  }
  let saved: { covered_until: number } | undefined;
  try {
    saved = JSON.parse(await readFile(checkpointPath, "utf8"));
  } catch (e: any) {
    if (e.code !== "ENOENT") throw e;
  }
  const to = Date.now() - 30_000;
  let from = saved ? saved.covered_until - 120_000 : to - 3_600_000;
  if (!Number.isFinite(from) || from < to - 29 * 86400000 || from > to)
    throw Error(
      "Checkpoint is invalid or older than retention; explicit backfill decision required",
    );
  let accepted = 0;
  while (from < to && !stopped) {
    const end = Math.min(from + 300_000, to);
    for (const worker of workers) accepted += await collect(worker, from, end);
    await writeFile(
      checkpointPath + ".next",
      JSON.stringify({
        covered_until: end,
        observed_at: Date.now(),
        counts: "sampled observations",
      }),
      { mode: 0o600 },
    );
    await rename(checkpointPath + ".next", checkpointPath);
    from = end;
  }
  console.log(
    `${new Date().toISOString()} ${accepted} new observations; coverage queried through ${new Date(from).toISOString()} (upstream sampling applies)`,
  );
}
try {
  do {
    try {
      await poll();
    } catch (error) {
      console.error(
        error instanceof Error ? error.message : "Collector failed",
      );
      if (!process.argv.includes("--watch")) process.exitCode = 1;
    }
    if (!process.argv.includes("--watch") || stopped) break;
    for (let i = 0; i < 30 && !stopped; i++) await Bun.sleep(1000);
  } while (!stopped);
} finally {
  await unlink(lockPath);
}
