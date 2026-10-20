import { test, expect } from "bun:test";
import { mapEvent, redact } from "./poe-errors-core";
const base = {
  timestamp: 1800000000000,
  $metadata: { id: "event-1", traceId: "trace" },
  $workers: { scriptVersion: { id: "release" } },
  source: { message: "Failure" },
};
test("maps stable IDs and deployment evidence without identity payloads", () => {
  const event = mapEvent(
    {
      ...base,
      source: { message: "Failure", userId: "private-user", cookie: "secret" },
    },
    "edge",
  );
  expect(event.event_id).toBe("event-1");
  expect(event.release).toBe("release");
  expect(JSON.stringify(event)).not.toContain("private-user");
});
test("client batches preserve coalesced frequency", () => {
  const event = mapEvent(
    {
      ...base,
      source: {
        message: "[client-errors]",
        code: "chat-failed",
        count: 17,
        details: { stack: "at chat (index.js:12:34)" },
      },
    },
    "edge",
  );
  expect(event.count).toBe(17);
  expect(event.source).toBe("edge:browser");
  expect(event.message).toBe("chat-failed");
});
test("volatile IDs and stack line changes keep the fingerprint", () => {
  const a = mapEvent(
    {
      ...base,
      source: { message: "GET /chat/123456 failed\n at send (index.js:12:34)" },
    },
    "edge",
  );
  const b = mapEvent(
    {
      ...base,
      source: { message: "GET /chat/654321 failed\n at send (index.js:80:90)" },
    },
    "edge",
  );
  expect(a.fingerprint).toBe(b.fingerprint);
  expect(a.message).not.toBe(b.message);
});
test("redacts credentials and URL query strings", () => {
  expect(
    redact("Bearer abc token=xyz a@example.com https://host/path?token=abc"),
  ).toBe("Bearer [redacted] token=[redacted] [email] https://host/path");
});
test("refuses unidentifiable upstream rows", () => {
  expect(() => mapEvent({ timestamp: 1 }, "edge")).toThrow("stable ID");
});
test("client serialized log shape keeps count and drops arbitrary details", () => {
  const event = mapEvent(
    {
      ...base,
      source: {
        message:
          "[client-errors] " +
          JSON.stringify({
            code: "offline",
            count: 4,
            message: "Fetch failed",
            details: { text: "private conversation" },
          }),
      },
    },
    "edge",
  );
  expect(event.count).toBe(4);
  expect(event.message).toBe("Fetch failed");
  expect(JSON.stringify(event)).not.toContain("private conversation");
});
