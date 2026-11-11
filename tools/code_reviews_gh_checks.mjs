// Run native gh against a private Unix socket. Count every upstream mutation.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  copyFileSync,
  symlinkSync,
  rmSync,
  chmodSync,
} from "node:fs";
import { join, resolve } from "node:path";
const [boss, hey, native] = process.argv.slice(2).map((p) => resolve(p));
const root = mkdtempSync(resolve(".cr-")),
  bin = join(root, "bin"),
  dispatch = join(bin, ".hey-gh-dispatch"),
  config = join(root, "config"),
  sock = join(root, "hub.sock");
mkdirSync(dispatch, { recursive: true });
mkdirSync(config);
copyFileSync("packages/hey-gh/gh", join(bin, "gh"));
chmodSync(join(bin, "gh"), 0o755);
symlinkSync(native, join(dispatch, "native"));
symlinkSync(boss, join(bin, "hey-boss"));
symlinkSync(hey, join(bin, "hey-gh"));
writeFileSync(
  join(config, "config.yml"),
  "http_unix_socket: " +
    sock +
    "\nprompt: disabled\naliases:\n  review-it: pr review\n",
);
const project = "github.com/example/time",
  sha = "a".repeat(40),
  writes = [],
  repo = "example/time";
const env = {
  ...process.env,
  PATH: bin + ":" + process.env.PATH,
  GH_CONFIG_DIR: config,
  GH_HOST: "github.com",
  GH_TOKEN: "fixture-personal",
  HEY_GH_APP_TOKEN: "fixture-app",
  GH_NO_UPDATE_NOTIFIER: "1",
  GH_NO_EXTENSION_UPDATE_NOTIFIER: "1",
  HEY_BOSS_AGENT_ID: "human:test",
  HEY_BOSS_ISSUE_DB: join(root, "issue.db"),
  HEY_BOSS_FLEET_STATE: root,
  TMPDIR: root,
};
for (const key of [
  "HEY_BOSS_ISSUE_HOST",
  "HEY_BOSS_ISSUE_PROJECT",
  "AGENT_EVAL",
  "HEY_GH_NATIVE_CALL",
])
  delete env[key];
const run = (name, args, input) =>
  new Promise((yes, no) => {
    const child = spawn(name, args, {
      env,
      cwd: root,
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "",
      stderr = "";
    child.stdout.on("data", (b) => (stdout += b));
    child.stderr.on("data", (b) => (stderr += b));
    child.stdin.end(input);
    child.on("error", no);
    child.on("exit", (code) => yes({ code, stdout, stderr }));
  });
const ok = async (...args) => {
  const r = await run(...args);
  assert.equal(r.code, 0, r.stderr + r.stdout);
  return r.stdout;
};
const hub = createServer(async (req, res) => {
  let bytes = "";
  for await (const b of req) bytes += b;
  const path = req.url.split("?")[0],
    val = bytes ? JSON.parse(bytes) : {};
  const mut =
    req.method !== "GET" &&
    !(path === "/graphql" && !/\bmutation\b/.test(val.query));
  if (mut) writes.push({ path, auth: req.headers.authorization });
  let result = {};
  if (path === "/graphql") {
    const pr = {
      id: "PR_fixture",
      number: 7,
      title: "Timezone fix",
      headRefOid: sha,
      reviews: { nodes: [], pageInfo: { hasNextPage: false } },
    };
    result = {
      data: {
        repository: {
          id: "R_fixture",
          name: "time",
          owner: { login: "example" },
          pullRequest: pr,
        },
        node: { ...pr, repository: { nameWithOwner: "example/time" } },
      },
    };
  } else if (path.endsWith("/files"))
    result = [
      {
        filename: "time.rs",
        patch: "@@ -8,2 +8,3 @@\n before\n+midnight();\n after",
      },
    ];
  else if (path.endsWith("/reviews") || path.endsWith("/comments"))
    result = mut ? { id: 903 } : [];
  else if (path.endsWith("/pulls/7"))
    result = { state: "open", title: "Timezone fix", head: { sha } };
  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify(result));
});
await new Promise((yes, no) => hub.listen(sock, yes).once("error", no));
try {
  await ok(boss, [
    "project",
    "init",
    "--project",
    project,
    "--prs",
    "true",
    "--worktree",
    "false",
    "--yes",
    "--json",
  ]);
  await ok(boss, [
    "issue",
    "--project",
    project,
    "settings",
    "set",
    "--code-reviews-enabled",
    "--json",
  ]);
  const gh = join(bin, "gh"),
    api = "repos/" + repo + "/pulls/7";
  const inline = JSON.parse(
    await ok(
      gh,
      ["api", api + "/comments", "--input", "-"],
      JSON.stringify({
        path: "time.rs",
        line: 9,
        side: "RIGHT",
        commit_id: sha,
        body: "Test midnight.",
      }),
    ),
  );
  assert.equal(inline.line, 9);
  assert.match(inline.diff_hunk, /midnight/);
  await ok(
    gh,
    [
      "review-it",
      "https://github.com/example/time/pull/7",
      "--request-changes",
      "--body-file",
      "-",
    ],
    "Please cover midnight.",
  );
  let reviews = JSON.parse(await ok(gh, ["api", api + "/reviews"]));
  assert.equal(reviews.length, 1);
  assert.equal(reviews[0].state, "CHANGES_REQUESTED");
  const node = JSON.parse(
    await ok(
      gh,
      ["api", "graphql", "--input", "-"],
      JSON.stringify({
        query:
          "mutation($input:AddPullRequestReviewInput!){draft:addPullRequestReview(input:$input){pullRequestReview{id state}}}",
        variables: {
          input: { pullRequestId: "PR_fixture", body: "Next pass" },
        },
      }),
    ),
  );
  assert.equal(node.data.draft.pullRequestReview.state, "PENDING");
  const pending = node.data.draft.pullRequestReview.id;
  const thread = JSON.parse(
    await ok(
      gh,
      ["api", "graphql", "--input", "-"],
      JSON.stringify({
        query:
          "mutation($input:AddPullRequestReviewThreadInput!){addPullRequestReviewThread(input:$input){thread{path line comments{totalCount}}}}",
        variables: {
          input: {
            pullRequestReviewId: pending,
            path: "time.rs",
            line: 9,
            side: "RIGHT",
            body: "Looks good.",
          },
        },
      }),
    ),
  );
  assert.equal(thread.data.addPullRequestReviewThread.thread.line, 9);
  await ok(
    gh,
    ["api", "graphql", "--input", "-"],
    JSON.stringify({
      query:
        "mutation($input:SubmitPullRequestReviewInput!){submitPullRequestReview(input:$input){pullRequestReview{state}}}",
      variables: {
        input: {
          pullRequestReviewId: pending,
          event: "APPROVE",
          body: "The boundary is covered.",
        },
      },
    }),
  );
  const bad = await run(gh, [
    "api",
    api + "/comments",
    "-f",
    "path=time.rs",
    "-F",
    "line=300",
    "-f",
    "commit_id=" + sha,
    "-f",
    "body=Bad anchor",
  ]);
  assert.notEqual(bad.code, 0);
  const view = JSON.parse(
    await ok(gh, ["pr", "view", "7", "-R", repo, "--json", "number,reviews"]),
  );
  assert.ok(view.reviews.some((r) => r.state === "APPROVED"));
  assert.equal(writes.length, 0, "Every review operation stays local");
  await ok(hey, [
    "api",
    "repos/elsewhere/other/issues/8/comments",
    "-f",
    "body=Normal comment",
  ]);
  assert.match(writes[0].auth, /fixture-app$/);
  await ok(gh, [
    "api",
    "repos/elsewhere/other/pulls/1/reviews",
    "-f",
    "event=APPROVE",
  ]);
  assert.match(writes[1].auth, /fixture-personal$/);
  console.log(
    "Native gh: stdin, aliases, body-file, REST inline comments, review submission, GraphQL drafts and threads, readback, invalid anchors, disabled projects and both identities passed.",
  );
} finally {
  await new Promise((r) => hub.close(r));
  rmSync(root, { recursive: true, force: true });
}
