import { createCommitReviewModel } from './diff.js';
import { renderGitFiles } from './git-page.js';
import { mountGitFiles } from './git-files.js';
import { mountGitLayout, renderGitLayoutControl } from './git-layout.js';

const $ = selector => document.querySelector(selector);
let commit, message;
function renderMessage({ raw, message: fullMessage }) {
  ({ commit } = createCommitReviewModel({}, raw));
  message = fullMessage;
  document.title = `${commit.shortHash} · ${commit.subject}`;
  $('h1').textContent = commit.subject;
  $('.review-byline').textContent = `${commit.hash} · ${commit.author} · ${new Date(commit.authoredAt).toLocaleString()}`;
  $('#message').textContent = commit.body;
}
try {
  const response = await fetch('commit.json');
  if (!response.ok) throw new Error('This viewer has expired. Run g open again.');
  const result = await response.json();
  const { files, summary } = createCommitReviewModel({}, result.raw);
  renderMessage(result);
  $('#edit-message').hidden = !!result.edit_error;
  $('#edit-help').textContent = result.edit_error || '';
  $('#edit-branch').textContent = `Saving rewrites this commit and any newer commits on ${result.branch.replace(/^refs\/heads\//, '')}.`;
  $('#summary').textContent = `${summary.files} changed files · +${summary.additions} −${summary.deletions}`;
  $('#layout').innerHTML = renderGitLayoutControl();
  $('#files').innerHTML = renderGitFiles({ files });
  mountGitFiles(); mountGitLayout();
  $('#status').textContent = '';
  const form = $('#message-form'), input = $('#commit-message'), save = $('#save-message'), cancel = $('#cancel-message');
  function closeEditor() { form.hidden = true; $('#message-reader').hidden = false; $('#edit-message').focus(); }
  $('#edit-message').onclick = () => {
    input.value = message.trimEnd(); $('#message-error').textContent = '';
    $('#message-reader').hidden = true; form.hidden = false; input.focus(); input.setSelectionRange(0, 0);
  };
  cancel.onclick = closeEditor;
  form.onkeydown = event => {
    if (event.key === 'Escape' && !cancel.disabled) { event.preventDefault(); closeEditor(); }
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey) && !save.disabled) { event.preventDefault(); form.requestSubmit(); }
  };
  form.onsubmit = async event => {
    event.preventDefault(); if (save.disabled) return;
    save.disabled = cancel.disabled = input.disabled = true; $('#message-error').textContent = '';
    try {
      const response = await fetch('message', { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Git-Shelf-Review': location.pathname }, body: JSON.stringify({ sha: commit.hash, message: input.value }) });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error || 'Could not save commit message');
      renderMessage(result); closeEditor(); $('#status').textContent = 'Commit message saved';
    } catch (error) { $('#message-error').textContent = error.message; }
    finally { save.disabled = cancel.disabled = input.disabled = false; }
  };
} catch (error) {
  $('#status').textContent = error.message;
}
