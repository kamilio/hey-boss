import { gitLineNumberDigits } from "./git-rows.js";
import { renderGitFileToggle } from "./git-files.js";

function escapeHtml(value) { return String(value ?? "").replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character]); }

function highlightCode(source, filename) {
  if (!/\.(?:[cm]?[jt]sx?|json|css|html?|md|py|rb|go|rs|java|c|cc|cpp|h|hpp|sh|ya?ml)$/i.test(filename)) return escapeHtml(source);
  const tokens = /("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`|\/\/.*|#\s.*|\b(?:async|await|break|case|catch|class|const|continue|def|delete|do|else|end|export|extends|false|finally|for|from|func|function|if|implements|import|in|interface|let|match|new|null|package|private|protected|public|return|static|struct|switch|throw|true|try|typeof|undefined|var|while|yield)\b|\b\d+(?:\.\d+)?\b)/g;
  let html = "", cursor = 0;
  for (const match of source.matchAll(tokens)) {
    html += escapeHtml(source.slice(cursor, match.index));
    const value = match[0], kind = value.startsWith("//") || value.startsWith("# ") ? "comment" : /^["'`]/.test(value) ? "string" : /^\d/.test(value) ? "number" : "keyword";
    html += `<span class="syntax-${kind}">${escapeHtml(value)}</span>`; cursor = match.index + value.length;
  }
  return html + escapeHtml(source.slice(cursor));
}

function lineHtml(line, file) {
    const side = line.type === "deletion" ? "old" : "new", number = side === "old" ? line.oldLine : line.newLine;
    return `<div class="diff-row ${line.type}" data-file="${escapeHtml(file.path)}" data-side="${side}" data-line="${number}" data-old-line="${line.oldLine ?? ""}"><button type="button" class="add-comment" aria-label="Comment on ${escapeHtml(file.path)} line ${number}" title="Comment on ${escapeHtml(file.path)}:${number}">+</button><span class="old-line">${line.oldLine ?? ""}</span><span class="new-line">${line.newLine ?? ""}</span><code>${line.type === "addition" ? "+" : line.type === "deletion" ? "-" : " "}${highlightCode(line.content, file.path)}</code></div>`;
}

function splitHunk(hunk, file, renderLine) {
  const old = [], next = [];
  for (let index = 0; index < hunk.lines.length;) {
    if (hunk.lines[index].type === "context") { old.push(renderLine(hunk.lines[index])); next.push(renderLine(hunk.lines[index])); index++; continue; }
    const deletions = [], additions = [];
    while (index < hunk.lines.length && hunk.lines[index].type !== "context") { (hunk.lines[index].type === "deletion" ? deletions : additions).push(hunk.lines[index]); index++; }
    const count = Math.max(deletions.length, additions.length);
    for (let row = 0; row < count; row++) { old.push(deletions[row] ? renderLine(deletions[row]) : '<div class="diff-row placeholder"></div>'); next.push(additions[row] ? renderLine(additions[row]) : '<div class="diff-row placeholder"></div>'); }
  }
  return `<div class="hunk">${escapeHtml(hunk.header)}</div><div class="split-pair"><div data-diff-side="Old" tabindex="0" role="group" aria-label="Old lines in ${escapeHtml(file.path)}">${old.join("")}</div><div data-diff-side="New" tabindex="0" role="group" aria-label="New lines in ${escapeHtml(file.path)}">${next.join("")}</div></div>`;
}

function fileHtml(file, context = {}) {
  const renderedLines = new Map();
  const renderLine = (line) => { if (!renderedLines.has(line)) renderedLines.set(line, lineHtml(line, file)); return renderedLines.get(line); };
  const unified = file.hunks.map((hunk) => `<div class="hunk">${escapeHtml(hunk.header)}</div>${hunk.lines.map(renderLine).join("")}`).join("");
  const split = file.hunks.map((hunk) => splitHunk(hunk, file, renderLine)).join("");
  const content = file.binary ? '<div class="empty">Binary file not shown</div>' : unified ? `<div class="unified-view" tabindex="0" role="group" aria-label="Unified lines in ${escapeHtml(file.path)}">${unified}</div><div class="split-view">${split}</div>` : '<div class="empty">No textual changes</div>';
  return `<article class="file" data-review-context="${escapeHtml(JSON.stringify(context))}" style="--git-line-number-digits:${gitLineNumberDigits(file)}"><header>${renderGitFileToggle()}<strong>${escapeHtml(file.status === "renamed" ? `${file.oldPath} → ${file.path}` : file.path)}</strong><span>+${file.additions} −${file.deletions}</span></header><div class="file-content">${content}</div></article>`;
}

export function renderGitFiles(model) {
  if (model.kind === "git-range") return model.commits.map((commit) => `<section class="commit"><h2>${escapeHtml(commit.commit.shortHash)} · ${escapeHtml(commit.commit.subject)}</h2>${commit.files.map((file) => fileHtml(file, { commit: commit.commit.hash })).join("")}</section>`).join("");
  if (model.kind === "git-worktree") return model.sections.map((section) => `<section><h2>${escapeHtml(section.label)}</h2>${section.files.map((file) => fileHtml(file, { section: section.label })).join("")}</section>`).join("");
  return (model.files ?? []).map((file) => fileHtml(file)).join("");
}
