// The adapter owns Poe-specific mapping. The inbox accepts only this generic shape.
export type ErrorEvent = {
  source: string;
  event_id: string;
  timestamp: number;
  message: string;
  stack?: string;
  release?: string;
  request_ref?: string;
  fingerprint: string;
  count: number;
};
export function redact(text: string): string {
  return text
    .replace(/Bearer\s+\S+/gi, "Bearer [redacted]")
    .replace(/\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/gi, "[email]")
    .replace(/https?:\/\/[^\s"<>]+/g, (url) => {
      try {
        const u = new URL(url);
        return `${u.origin}${u.pathname}`;
      } catch {
        return "[url]";
      }
    })
    .replace(
      /((?:token|password|secret|authorization|cookie|api[_-]?key)["\x27]?\s*[=:]\s*["\x27]?)[^\s,;}"\x27]+/gi,
      "$1[redacted]",
    );
}
export function normalize(text: string): string {
  return text
    .replace(/[0-9a-f]{8}-[0-9a-f-]{27,}/gi, "<uuid>")
    .replace(/\b[0-9a-f]{24,}\b/gi, "<hash>")
    .replace(/\b\d{6,}\b/g, "<id>")
    .replace(/:\d+:\d+/g, ":<line>:<column>")
    .replace(/\s+/g, " ")
    .trim();
}
export function mapEvent(row: any, worker: string): ErrorEvent {
  const meta = row.$metadata ?? {},
    src = row.source ?? {},
    runtime = row.$workers ?? {};
  if (typeof meta.id !== "string" || !meta.id)
    throw Error("Cloudflare event has no stable ID");
  const timestamp =
    typeof row.timestamp === "number"
      ? row.timestamp
      : Date.parse(row.timestamp);
  if (!Number.isFinite(timestamp))
    throw Error("Cloudflare event has no valid timestamp");
  let fields = src;
  const raw = String(
    src.message ?? meta.error ?? meta.message ?? "Worker error",
  );
  if (raw.startsWith("[client-errors] ")) {
    try {
      fields = { ...src, ...JSON.parse(raw.slice(16)) };
    } catch {
      /* Keep original bounded reason. */
    }
  }
  const client = raw.startsWith("[client-errors]");
  const code = typeof fields.code === "string" ? fields.code : undefined;
  const reason = client
    ? typeof fields.message === "string" &&
      !fields.message.startsWith("[client-errors]")
      ? fields.message
      : (code ?? "Browser error")
    : raw;
  const context = [
    fields.job,
    fields.outcome,
    fields.operation,
    fields.errorMessage,
  ]
    .filter((v) => typeof v === "string")
    .join(" · ");
  const message = redact(`${reason}${context ? " · " + context : ""}`).slice(
    0,
    8192,
  );
  const stack = redact(
    String(
      fields.details?.stack ??
        fields.stack ??
        (reason.includes("\n") ? reason : ""),
    ),
  ).slice(0, 16384);
  const path = normalize(String(runtime.event?.request?.path ?? ""));
  const stable = normalize(message.split("\n")[0]);
  const frames = stack
    .split("\n")
    .filter((s) => /^\s*at\s/.test(s))
    .slice(0, 3)
    .map((s) => s.trim().replace(/\s+\(.*/, ""))
    .join(" | ");
  const count = client ? (fields.count ?? 1) : 1;
  if (!Number.isSafeInteger(count) || count < 1 || count > 1_000_000)
    throw Error("Invalid coalesced client count");
  const release =
    fields.pageRelease?.commitSha ??
    fields.pageRelease?.releaseId ??
    runtime.scriptVersion?.id;
  return {
    source: `${worker}:${client ? "browser" : "worker"}`,
    event_id: meta.id,
    timestamp,
    message,
    ...(stack ? { stack } : {}),
    ...(typeof release === "string" ? { release: release.slice(0, 256) } : {}),
    ...(typeof (meta.traceId ?? meta.rayId ?? meta.requestId) === "string"
      ? {
          request_ref: String(
            meta.traceId ?? meta.rayId ?? meta.requestId,
          ).slice(0, 512),
        }
      : {}),
    fingerprint: `v1|${code ?? ""}|${path}|${stable}|${normalize(frames)}`.slice(0, 1024),
    count,
  };
}
