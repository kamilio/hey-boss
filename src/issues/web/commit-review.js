import { parseUnifiedDiff } from './git-review/diff.js';
import { renderGitFiles } from './git-review/git-page.js';
import { mountGitComments } from './git-review/git-comments.js';
import { mountGitFiles } from './git-review/git-files.js';
import { mountGitLayout } from './git-review/git-layout.js';

const $ = selector => document.querySelector(selector);
const params = new URLSearchParams(location.search);
const project = params.get('project'), number = Number(params.get('issue')), commit = params.get('commit');
const host = document.documentElement.dataset.issueMobile === 'true' ? null : params.get('host');
let boot, issue, readyGuard, snapshot, busy = false;
const request = operation => {
  const read = operation.action === 'view' || (operation.action === 'commit_review' && ['view','patch','comments'].includes(operation.operation.command));
  return HeyBossUI.post('/api/action', {project, operation, ...(!read ? {request_id:HeyBossUI.requestId()} : {}), ...(host ? {host} : {})}, boot, {mobile:document.documentElement.dataset.issueMobile === 'true', fallback:'Could not save review'});
};
const review = operation => request({action:'commit_review',number,commit,operation});
const showError = error => { $('#review-error').textContent = error?.message || ''; $('#review-error').hidden = !error; };
function taskBar() {
  $('#task-title span:last-child').textContent = `#${number} · ${issue.title}`;
  $('#task-state').textContent = issue.state[0].toUpperCase() + issue.state.slice(1);
  $('#task-state').className = 'state-pill ' + issue.state;
  $('#task-owner').textContent = issue.assignee ? (issue.assignee_name || (issue.assignee === 'human:boss' ? 'You' : issue.assignee)) : 'Unassigned';
  const mine = issue.assignee === 'human:boss', assignable = ['open','ready'].includes(issue.state);
  $('#unassign').hidden = !issue.assignee || !assignable;
  $('#assign-me').hidden = mine || !assignable;
  $('#rework').hidden = issue.state === 'open';
  $('#rework').textContent = mine ? 'Reopen task' : 'Reopen and assign to me';
  for (const id of ['#unassign','#assign-me','#rework']) $(id).disabled = busy;
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
function renderMessage(bodyHtml) {
  const subject = snapshot.message.split('\n',1)[0];
  $('h1').textContent = subject; document.title = `${subject} · Hey Boss`;
  $('#message-body').innerHTML = bodyHtml; $('#full-message').hidden = !bodyHtml.trim();
}
async function start() {
  if (!project || !Number.isSafeInteger(number) || number < 1 || !commit) throw new Error('Open a commit from its task to review it.');
  $('.brand').href = `/#${new URLSearchParams({project,...(host?{host}:{})})}`;
  const response = await fetch('/api/bootstrap'); boot = await response.json();
  if (!response.ok || !boot.ok) throw new Error(boot.error?.message || 'Could not connect to Hey Boss');
  const [result, task] = await Promise.all([review({command:'view'}), request({action:'view',number})]);
  snapshot = result.review; issue = task.issue; readyGuard = task.ready_guard;
  const files = parseUnifiedDiff(snapshot.patch);
  $('#task-title').href = `/#${new URLSearchParams({project,issue:number,...(host?{host}:{})})}`;
  $('#commit-sha').textContent = snapshot.sha.slice(0,7);
  $('#commit-sha').title = snapshot.sha;
  $('#author').textContent = snapshot.author;
  const authored = new Date(snapshot.authored_at);
  $('#authored').textContent = Number.isNaN(authored.valueOf()) ? snapshot.authored_at : authored.toLocaleDateString(undefined,{year:'numeric',month:'short',day:'numeric'});
  $('#authored').dateTime = snapshot.authored_at;
  $('#authored').title = snapshot.authored_at;
  $('#files').innerHTML = renderGitFiles({files});
  $('#file-count').textContent = files.length;
  HeyBossUI.icons($('#review'));
  renderMessage(result.message_body_html);
  if (window !== window.top) $('#full-message').open = true;
  if (task.comments.length) $('#files').after(HeyBossUI.reviewDiscussion(task, project, host));
  $('#loading').remove(); $('#review').hidden = false;
  taskBar(); mountGitFiles(); mountGitComments({comments:result.comments,request:review}); mountGitLayout();
  $('#rework').onclick = () => action({action:'rework',number,if_version:issue.version});
  $('#unassign').onclick = () => action({action:'assign',number,target:'unassigned',if_version:issue.version});
  $('#assign-me').onclick = () => action({action:'assign',number,target:'boss',if_version:issue.version});
  $('#make-ready').onclick = () => action({action:'ready',number,force:false,guard:readyGuard});
  $('#refresh-task').onclick = () => action({action:'view',number});
  const form = $('#message-form'), input = $('#commit-message');
  function closeEditor() {
    form.hidden = true; $('#message-reader').hidden = false;
    $('#message-error').textContent = ''; $('#edit-message').focus();
  }
  $('#edit-message').onclick = () => {
    $('#message-reader').hidden = true; form.hidden = false; input.value = snapshot.message;
    input.setSelectionRange(0,0); input.focus();
  };
  $('#cancel-message').onclick = closeEditor;
  document.addEventListener('keydown', event => {
    if (event.key === 'Escape' && !form.hidden && !$('#cancel-message').disabled) {
      event.preventDefault(); closeEditor();
    }
  });
  form.onsubmit = async event => {
    event.preventDefault(); const message = input.value;
    input.readOnly = true;
    for (const button of form.querySelectorAll('button')) button.disabled = true;
    $('#message-error').textContent = '';
    try {
      const saved = await review({command:'message',message,previous_message:snapshot.message}); snapshot.message = message;
      renderMessage(saved.message_body_html);
      closeEditor(); $('#status').textContent = 'Message saved'; await refreshTask();
    } catch(error) { $('#message-error').textContent = error.message; }
    finally { input.readOnly = false; for (const button of form.querySelectorAll('button')) button.disabled = false; }
  };
  $('#download-patch').onclick = async () => {
    const button = $('#download-patch'); button.disabled = true; showError(null);
    try { const result = await review({command:'patch'}); const url = URL.createObjectURL(new Blob([result.patch],{type:'text/plain;charset=utf-8'})); const link = document.createElement('a'); link.href = url; link.download = `${snapshot.sha.slice(0,12)}.patch`; link.click(); setTimeout(()=>URL.revokeObjectURL(url),1000); }
    catch(error) { showError(error); } finally { button.disabled = false; }
  };
  // Comments revise the task version too; refresh guards without rebuilding editors.
  document.addEventListener('git-comment-activity', event => { if (event.detail.state === 'success') refreshTask().catch(showError); });
}
start().catch(error => { const main = $('#review-main'); const heading = document.createElement('h1'); heading.textContent = 'Could not open this review'; const message = document.createElement('p'); message.textContent = error.message; const link = document.createElement('a'); link.href = `/#${new URLSearchParams({project:project||'',issue:number||'',...(host?{host}:{})})}`; link.textContent = 'Back to task'; main.replaceChildren(heading,message,link); });
