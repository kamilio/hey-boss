import { createCommitReviewModel } from './diff.js';
import { renderGitFiles } from './git-page.js';
import { mountGitFiles } from './git-files.js';
import { mountGitLayout, renderGitLayoutControl } from './git-layout.js';

const $ = selector => document.querySelector(selector);
try {
  const response = await fetch('commit.json');
  if (!response.ok) throw new Error('This viewer has expired. Run g open again.');
  const { commit, files, summary } = createCommitReviewModel({}, await response.json());
  document.title = `${commit.shortHash} · ${commit.subject}`;
  $('h1').textContent = commit.subject;
  $('.review-byline').textContent = `${commit.hash} · ${commit.author} · ${new Date(commit.authoredAt).toLocaleString()}`;
  $('#message').textContent = commit.body;
  $('#summary').textContent = `${summary.files} changed files · +${summary.additions} −${summary.deletions}`;
  $('#layout').innerHTML = renderGitLayoutControl();
  $('#files').innerHTML = renderGitFiles({ files });
  mountGitFiles(); mountGitLayout();
  $('#status').remove();
} catch (error) {
  $('#status').textContent = error.message;
}
