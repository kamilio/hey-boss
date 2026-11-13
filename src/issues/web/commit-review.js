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
function renderMessage() {
  const [subject,...lines] = snapshot.message.split('\n'), body = lines.join('\n').trim();
  $('h1').textContent = subject; document.title = `${subject} · Hey Boss`;
  $('#message-body').textContent = body; $('#full-message').hidden = !body;
}
async function start() {
  if (!project || !Number.isSafeInteger(number) || number < 1 || !commit) throw new Error('Open a commit from its task to review it.');
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
  renderMessage();
  $('#loading').remove(); $('#review').hidden = false;
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
      renderMessage();
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
