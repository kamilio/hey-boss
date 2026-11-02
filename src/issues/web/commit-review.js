import { parseUnifiedDiff } from './git-review/diff.js';
import { renderGitReviewHtml } from './git-review/git-page.js';
import { mountGitComments } from './git-review/git-comments.js';
import { mountGitFiles } from './git-review/git-files.js';
import { mountGitLayout } from './git-review/git-layout.js';

const $ = selector => document.querySelector(selector);
const params = new URLSearchParams(location.search);
const project = params.get('project'), number = Number(params.get('issue')), commit = params.get('commit');
const host = document.documentElement.dataset.issueMobile === 'true' ? null : params.get('host');
let boot, issue, readyGuard, snapshot, busy = false;
const esc = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const request = operation => {
  const read = operation.action === 'view' || (operation.action === 'commit_review' && ['view','patch','comments'].includes(operation.operation.command));
  return HeyBossUI.post('/api/action', {project, operation, ...(!read ? {request_id:HeyBossUI.requestId()} : {}), ...(host ? {host} : {})}, boot, {mobile:document.documentElement.dataset.issueMobile === 'true', fallback:'Could not save review'});
};
const review = operation => request({action:'commit_review',number,commit,operation});
const showError = error => { $('#review-error').textContent = error?.message || ''; $('#review-error').hidden = !error; };
function taskBar() {
  $('#task-title').textContent = `#${number} · ${issue.title}`;
  $('#task-state').textContent = issue.state[0].toUpperCase() + issue.state.slice(1);
  $('#task-owner').textContent = issue.assignee ? (issue.assignee_name || (issue.assignee === 'human:boss' ? 'You' : issue.assignee)) : 'Unassigned';
  $('#unassign').disabled = busy || !issue.assignee || !['open','ready'].includes(issue.state);
  $('#assign-me').disabled = busy || issue.assignee === 'human:boss' || !['open','ready'].includes(issue.state);
  $('#rework').disabled = busy || (issue.state === 'open' && issue.assignee === 'human:boss');
  $('#make-ready').hidden = issue.state === 'ready'; $('#make-ready').disabled = busy;
  $('#refresh-task').disabled = busy;
}
async function refreshTask() { const task = await request({action:'view',number}); issue = task.issue; readyGuard = task.ready_guard; taskBar(); }
async function action(operation) {
  if (busy) return;
  busy = true; showError(null); taskBar();
  try { await request(operation); await refreshTask(); $('#status').textContent = 'Task updated'; }
  catch (error) { showError(error); }
  finally { busy = false; taskBar(); }
}
async function start() {
  if (!project || !Number.isSafeInteger(number) || number < 1 || !commit) throw new Error('Open a commit from its task to review it.');
  const response = await fetch('/api/bootstrap'); boot = await response.json();
  if (!response.ok || !boot.ok) throw new Error(boot.error?.message || 'Could not connect to Hey Boss');
  const [result, task] = await Promise.all([review({command:'view'}), request({action:'view',number})]);
  snapshot = result.review; issue = task.issue; readyGuard = task.ready_guard;
  const [subject, ...body] = snapshot.message.split('\n');
  const model = {commit:{hash:snapshot.sha,shortHash:snapshot.sha.slice(0,7),subject,body:body.join('\n').trim(),author:snapshot.author,authoredAt:snapshot.authored_at},files:parseUnifiedDiff(snapshot.patch),comments:result.comments};
  const rendered = new DOMParser().parseFromString(renderGitReviewHtml(model),'text/html');
  // Use the renderer's escaped DOM and mount its controllers with our authenticated API.
  document.body.replaceChildren(...rendered.body.querySelectorAll('header.bar,main'));
  document.title = `${subject} · Hey Boss`;
  const bar = $('.bar');
  const back = `/#${new URLSearchParams({project,issue:number,...(host?{host}:{})})}`;
  bar.querySelector('.mark').outerHTML = `<a class="mark" href="${esc(back)}" aria-label="Back to task">← Task</a>`;
  bar.querySelector('strong').textContent = 'Commit review';
  bar.insertAdjacentHTML('afterend', `<section class="task-bar" aria-label="Task actions"><div class="task-info"><a id="task-title" href="${esc(back)}"></a><div class="task-meta"><span id="task-state"></span><span id="task-owner"></span></div></div><div class="task-actions"><button id="rework" type="button">Not Ready · Assign to me</button><button id="assign-me" type="button">Assign to me</button><button id="make-ready" type="button">Ready</button><button id="unassign" type="button" class="primary">Unassign</button><button id="refresh-task" type="button" title="Refresh task status" aria-label="Refresh task status">↻</button></div></section><div id="review-error" class="review-error" role="alert" hidden></div>`);
  const summary = $('.summary');
  summary.insertAdjacentHTML('beforeend', `<div class="message-tools"><code>${esc(snapshot.sha)}</code><button id="edit-message" type="button">Edit commit message</button><button id="download-patch" type="button">Download patch</button></div><form id="message-form" hidden><label for="commit-message">Commit message</label><textarea id="commit-message" required maxlength="65536" spellcheck="false"></textarea><div class="message-actions"><span id="message-error" role="alert"></span><button type="button" id="cancel-message">Cancel</button><button type="submit" class="primary">Save message</button></div></form>`);
  taskBar(); mountGitFiles(); mountGitComments({comments:result.comments,request:review}); mountGitLayout();
  $('#rework').onclick = () => action({action:'rework',number,if_version:issue.version});
  $('#unassign').onclick = () => action({action:'assign',number,target:'unassigned',if_version:issue.version});
  $('#assign-me').onclick = () => action({action:'assign',number,target:'boss',if_version:issue.version});
  $('#make-ready').onclick = () => action({action:'ready',number,force:false,guard:readyGuard});
  $('#refresh-task').onclick = () => action({action:'view',number});
  $('#edit-message').onclick = () => { $('#message-form').hidden = false; $('#commit-message').value = snapshot.message; $('#commit-message').focus(); $('#edit-message').hidden = true; };
  $('#cancel-message').onclick = () => { $('#message-form').hidden = true; $('#edit-message').hidden = false; $('#message-error').textContent = ''; $('#edit-message').focus(); };
  $('#message-form').onsubmit = async event => {
    event.preventDefault(); const form = event.currentTarget, message = $('#commit-message').value;
    for (const button of form.querySelectorAll('button')) button.disabled = true;
    $('#message-error').textContent = '';
    try {
      await review({command:'message',message,previous_message:snapshot.message}); snapshot.message = message;
      const [subject,...body] = message.split('\n'); $('.summary h1').textContent = subject; $('.summary p').textContent = body.join('\n').trim(); document.title = `${subject} · Hey Boss`; $('.review-title').textContent = `${snapshot.sha.slice(0,7)} · ${subject}`;
      $('#message-form').hidden = true; $('#edit-message').hidden = false; $('#status').textContent = 'Message saved'; $('#edit-message').focus(); await refreshTask();
    } catch(error) { $('#message-error').textContent = error.message; }
    finally { for (const button of form.querySelectorAll('button')) button.disabled = false; }
  };
  $('#download-patch').onclick = async () => {
    const button = $('#download-patch'); button.disabled = true; showError(null);
    try { const result = await review({command:'patch'}); const url = URL.createObjectURL(new Blob([result.patch],{type:'text/plain;charset=utf-8'})); const link = document.createElement('a'); link.href = url; link.download = `${snapshot.sha.slice(0,12)}.patch`; link.click(); setTimeout(()=>URL.revokeObjectURL(url),1000); }
    catch(error) { showError(error); } finally { button.disabled = false; }
  };
  // Comments revise the task version too; refresh guards without rebuilding editors.
  document.addEventListener('git-comment-activity', event => { if (event.detail.state === 'success') refreshTask().catch(showError); });
}
start().catch(error => { document.body.replaceChildren(); const main = document.createElement('main'); const heading = document.createElement('h1'); heading.textContent = 'Could not open this review'; const message = document.createElement('p'); message.textContent = error.message; const link = document.createElement('a'); link.href = `/#${new URLSearchParams({project:project||'',issue:number||'',...(host?{host}:{})})}`; link.textContent = 'Back to task'; main.append(heading,message,link); document.body.append(main); });
