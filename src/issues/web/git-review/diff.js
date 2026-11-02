function decodeGitPath(value = "") {
  const text = value.replace(/\t$/, "");
  if (!(text.startsWith('"') && text.endsWith('"'))) return text;
  const content = text.slice(1, -1), bytes = [], encoder = new TextEncoder(); let offset = 0;
  const escapes = { '"': 34, "\\": 92, a: 7, b: 8, f: 12, n: 10, r: 13, t: 9, v: 11 };
  for (const match of content.matchAll(/\\([0-7]{1,3}|.)/g)) {
    bytes.push(...encoder.encode(content.slice(offset, match.index)));
    if (/^[0-7]/.test(match[1])) bytes.push(parseInt(match[1], 8));
    else if (Object.hasOwn(escapes, match[1])) bytes.push(escapes[match[1]]);
    else bytes.push(...encoder.encode(match[0]));
    offset = match.index + match[0].length;
  }
  bytes.push(...encoder.encode(content.slice(offset)));
  return new TextDecoder().decode(new Uint8Array(bytes));
}

function displayPath(value) { return decodeGitPath(value).replace(/^[ab]\//, ""); }

export function parseUnifiedDiff(patch) {
  const files = []; let file = null; let hunk = null; let oldLine = 0; let newLine = 0; let oldLeft = 0; let newLeft = 0;
  for (const line of patch.split("\n")) {
    if (line.startsWith("diff --git ")) {
      const match = /^diff --git (.+?) (.+)$/.exec(line); const oldPath = match ? displayPath(match[1]) : ""; const newPath = match ? displayPath(match[2]) : oldPath;
      file = { oldPath, path: newPath, status: "modified", additions: 0, deletions: 0, hunks: [], binary: false }; files.push(file); hunk = null; oldLeft = 0; newLeft = 0; continue;
    }
    if (!file) continue;
    if (line.startsWith("new file mode ")) { file.status = "added"; continue; }
    if (line.startsWith("deleted file mode ")) { file.status = "deleted"; continue; }
    if (line.startsWith("rename from ")) { file.status = "renamed"; file.oldPath = decodeGitPath(line.slice(12)); continue; }
    if (line.startsWith("rename to ")) { file.path = decodeGitPath(line.slice(10)); continue; }
    if (line.startsWith("copy from ")) { file.status = "copied"; file.oldPath = decodeGitPath(line.slice(10)); continue; }
    if (line.startsWith("copy to ")) { file.path = decodeGitPath(line.slice(8)); continue; }
    if (line.startsWith("Binary files ") || line.startsWith("GIT binary patch")) { file.binary = true; continue; }
    if (!oldLeft && !newLeft && line.startsWith("--- ")) { const candidate = line.slice(4); if (candidate !== "/dev/null") file.oldPath = displayPath(candidate); continue; }
    if (!oldLeft && !newLeft && line.startsWith("+++ ")) { const candidate = line.slice(4); if (candidate !== "/dev/null") file.path = displayPath(candidate); continue; }
    const header = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$/.exec(line);
    if (header) { oldLine = Number(header[1]); newLine = Number(header[3]); oldLeft = Number(header[2] ?? 1); newLeft = Number(header[4] ?? 1); hunk = { header: line, context: header[5].trim(), lines: [] }; file.hunks.push(hunk); continue; }
    if (!hunk || (!oldLeft && !newLeft) || line === "\\ No newline at end of file") continue;
    if (line.startsWith("+")) { hunk.lines.push({ type: "addition", oldLine: null, newLine: newLine++, content: line.slice(1) }); file.additions++; newLeft--; }
    else if (line.startsWith("-")) { hunk.lines.push({ type: "deletion", oldLine: oldLine++, newLine: null, content: line.slice(1) }); file.deletions++; oldLeft--; }
    else if (line.startsWith(" ")) { hunk.lines.push({ type: "context", oldLine: oldLine++, newLine: newLine++, content: line.slice(1) }); oldLeft--; newLeft--; }
  }
  return files;
}

export function createCommitReviewModel(repository, raw) {
  const marker = "\u001eGIT_SHELF_META\u001e"; const split = raw.indexOf(marker);
  if (split < 0) throw new Error("Git returned an incomplete commit review.");
  const metadata = raw.slice(0, split).split("\u0000"); const patch = raw.slice(split + marker.length).replace(/^\n/, "");
  const [hash, shortHash, author, email, authoredAt, subject, body] = metadata; const files = parseUnifiedDiff(patch);
  return { version: 1, repository: { root: repository.root, branch: repository.branch, baseRef: repository.baseRef }, commit: { hash, shortHash, author, email, authoredAt, subject, body: body?.trim() ?? "" }, summary: { files: files.length, additions: files.reduce((n, file) => n + file.additions, 0), deletions: files.reduce((n, file) => n + file.deletions, 0) }, files };
}
