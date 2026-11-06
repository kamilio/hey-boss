"use strict";
let projectSettingsProject = null,
  projectSettingsFocus = null,
  projectPromptSections = [],
  projectSettingsOriginal = null,
  projectSettingsSaving = false,
  projectSettingsSequence = 0;
const projectSettingsTabs = [...document.querySelectorAll('.project-settings-tabs [role="tab"]')];
function selectProjectSettingsTab(tab, focus = false) {
  for (const item of projectSettingsTabs) {
    const selected = item === tab;
    item.setAttribute("aria-selected", String(selected));
    item.tabIndex = selected ? 0 : -1;
    document.getElementById(item.getAttribute("aria-controls")).hidden = !selected;
  }
  document.getElementById(tab.getAttribute("aria-controls")).scrollTop = 0;
  if (tab.id === "settings-tab-activity") loadConfigHistory();
  $(".project-settings-body").scrollTop = 0;
  if (focus) tab.focus();
  tab.scrollIntoView({block: "nearest", inline: "nearest"});
}
for (const [index, tab] of projectSettingsTabs.entries()) {
  tab.onclick = () => selectProjectSettingsTab(tab);
  tab.onkeydown = event => {
    let next;
    if (event.key === "ArrowRight") next = (index + 1) % projectSettingsTabs.length;
    else if (event.key === "ArrowLeft") next = (index + projectSettingsTabs.length - 1) % projectSettingsTabs.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = projectSettingsTabs.length - 1;
    else return;
    event.preventDefault();
    selectProjectSettingsTab(projectSettingsTabs[next], true);
  };
}
// Native validation must be able to focus the first invalid field, even in a closed tab/details.
$("#project-settings-form").addEventListener("invalid", event => {
  if (event.target !== $("#project-settings-form :invalid")) {
    event.preventDefault();
    return;
  }
  const panel = event.target.closest('[role="tabpanel"]');
  if (panel) selectProjectSettingsTab(document.getElementById(panel.getAttribute("aria-labelledby")));
  for (let parent = event.target.parentElement; parent; parent = parent.parentElement) {
    if (parent.tagName === "DETAILS") parent.open = true;
  }
}, true);
function projectSettingsDraft() {
  return {
    worker_model: $("#project-worker-model").value.trim(),
    worker_reasoning: $("#project-worker-reasoning").value,
    worker_permissions: $("#project-worker-permissions").value,
    paused: $("#project-paused").checked,
    pause_reason: $("#project-paused").checked ? $("#project-pause-reason").value.trim() : "",
    worktree_enabled: $("#project-worktree").checked,
    prs_enabled: $("#project-prs").checked,
    drafts_enabled: $("#project-drafts").checked,
    artifacts_enabled: $("#project-artifacts").checked,
  };
}
function projectSettingsChanged() {
  $("#project-worker-model-inherit").hidden = !$("#project-worker-model").value;
  const permissions = $("#project-worker-permissions").value;
  $("#project-worker-permissions-help").textContent = permissions === "yolo" ? "Full access. No sandbox or approval requests." : permissions === "auto" ? "Sandboxed, with automatic approval review." : "Use this machine’s configured permissions.";
  $("#project-worker-permissions-help").classList.toggle("execution-yolo", permissions === "yolo");
  $("#project-pause-details").hidden = !$("#project-paused").checked;
  $("#project-pause-reason").required = $("#project-paused").checked;
  const changed =
    JSON.stringify(projectSettingsDraft()) !==
    JSON.stringify(projectSettingsOriginal);
  $("#project-settings-state").textContent = changed ? "Unsaved changes" : "";
  $("#project-settings-form button[type=submit]").disabled =
    projectSettingsSaving || !projectSettingsOriginal || !changed;
}
function closeProjectSettings() {
  if (projectSettingsSaving) return;
  ++projectSettingsSequence;
  ++configHistorySequence;
  ++configDetailSequence;
  $("#project-settings-dialog").close();
  projectSettingsFocus?.focus();
}
$("#project-settings-trigger").onclick = async event => {
  if (event?.metaKey || event?.ctrlKey || event?.shiftKey || event?.altKey) return;
  event?.preventDefault();
  if (!model.project) return;
  document.getElementById("project-more-links")?.hidePopover();
  const sequence = ++projectSettingsSequence;
  projectSettingsProject = model.project.id;
  for (const link of document.querySelectorAll("[data-prompt-file]")) {
    const params = new URLSearchParams({project:projectSettingsProject, file:link.dataset.promptFile});
    if (model.route?.host) params.set("host", model.route.host);
    link.href = "/prompts#" + params;
    link.onclick = event => {
      if (projectSettingsSaving || (!event.metaKey && !event.ctrlKey && projectSettingsOriginal &&
          JSON.stringify(projectSettingsDraft()) !== JSON.stringify(projectSettingsOriginal) &&
          !confirm("Discard unsaved project settings?"))) event.preventDefault();
    };
  }
  projectSettingsFocus = document.getElementById("project-more") || $("#project-settings-trigger");
  projectSettingsOriginal = null;
  $("#project-settings-name").textContent = model.project.name;
  $("#project-settings-name").title = model.project.name;
  $("#project-settings-error").hidden = true;
  $("#project-settings-state").textContent = "Loading…";
  setProjectSettingsDisabled(true);
  $("#project-settings-form button[type=submit]").disabled = true;
  $("#project-settings-dialog").showModal();
  selectProjectSettingsTab(projectSettingsTabs[0]);
  try {
    const value = await api(
      { action: "project_settings" },
      projectSettingsProject,
    );
    if (sequence !== projectSettingsSequence) return;
    projectPromptSections = value.prompt_sections || [];
    $("#project-worker-model").value = value.worker_model ?? "";
    $("#project-worker-reasoning").value = value.worker_reasoning ?? "";
    $("#project-worker-permissions").value = value.worker_permissions ?? "auto";
    $("#project-paused").checked = value.paused === true;
    $("#project-pause-reason").value = value.pause_reason || "";
    $("#project-prs").checked = value.prs_enabled;
    $("#project-worktree").checked = value.worktree_enabled;
    for (const resource of ["drafts", "artifacts"]) $("#project-" + resource).checked = value[resource + "_enabled"] !== false;
    projectSettingsOriginal = projectSettingsDraft();
    setProjectSettingsDisabled(false);
    projectSettingsChanged();
  } catch (error) {
    if (sequence !== projectSettingsSequence) return;
    $("#project-settings-state").textContent = "";
    $("#project-settings-error").textContent = error.message;
    $("#project-settings-error").hidden = false;
  }
};
$("#project-settings-close").onclick = closeProjectSettings;
document.getElementById("paused-project-settings")?.addEventListener("click", () => $("#project-settings-trigger").click());
$("#project-settings-cancel").onclick = closeProjectSettings;
$("#project-settings-dialog").addEventListener("cancel", (event) => {
  event.preventDefault();
  closeProjectSettings();
});
$("#project-settings-form").onsubmit = async (event) => {
  event.preventDefault();
  if (projectSettingsSaving || !projectSettingsOriginal) return;
  projectSettingsSaving = true;
  setProjectSettingsDisabled(true);
  projectSettingsChanged();
  $("#project-settings-state").textContent = "Saving…";
  try {
    const draft = projectSettingsDraft();
    const pauseChanged = draft.paused !== projectSettingsOriginal.paused;
    const executionChanged = ["worker_model", "worker_reasoning", "worker_permissions"].some(key => draft[key] !== projectSettingsOriginal[key]);
    const changes = Object.fromEntries(Object.entries(draft).filter(([key, value]) => value !== projectSettingsOriginal[key]));
    await mutate(
      {action: "project_config", operation: {command: "apply", changes}},
      projectSettingsProject,
    );
    projectSettingsOriginal = draft;
    $("#project-settings-error").hidden = true;
    projectSettingsSaving = false;
    closeProjectSettings();
    detailCache.clear();
    await refreshProjects(projectSettingsProject);
    await renderRoute();
    updateHeader();
    toast(pauseChanged ? draft.paused ? "Project paused. Current agents can finish." : "Project resumed. Workers can pick up issues." : executionChanged ? "Settings saved. Execution changes apply to the next task or scheduled run." : "Settings saved");
  } catch (error) {
    $("#project-settings-error").textContent = error.message;
    $("#project-settings-error").hidden = false;
  } finally {
    projectSettingsSaving = false;
    if (projectSettingsOriginal) setProjectSettingsDisabled(false);
    projectSettingsChanged();
  }
};
for (const resource of ["prs", "worktree", "drafts", "artifacts"]) $("#project-" + resource).onchange = projectSettingsChanged;
$("#project-paused").onchange = projectSettingsChanged;
$("#project-pause-reason").oninput = projectSettingsChanged;
for (const field of ["model", "reasoning", "permissions"]) $("#project-worker-" + field).addEventListener("input", projectSettingsChanged);
$("#project-worker-model-inherit").onclick = () => {
  $("#project-worker-model").value = "";
  projectSettingsChanged();
  $("#project-worker-model").focus();
};
function setProjectSettingsDisabled(disabled) {
  $("#project-settings-close").disabled = projectSettingsSaving;
  $("#project-settings-cancel").disabled = projectSettingsSaving;
  for (const input of document.querySelectorAll("#project-settings-form input, #project-settings-form textarea, #project-settings-form select, #project-worker-model-inherit")) input.disabled = disabled;
}

let configHistorySequence = 0, configDetailSequence = 0, configHistoryOffset = null, configHistoryBusy = false;
function configElement(tag, text, className) {
  const element = document.createElement(tag);
  if (text !== undefined) element.textContent = text;
  if (className) element.className = className;
  return element;
}
function configField(key) {
  if (key === 'prompt') return 'prompts/worker.md';
  if (key.startsWith('prompt_overrides.')) {
    const section = key.slice('prompt_overrides.'.length);
    return 'prompts/' + (projectPromptSections.find(s => s.key === section)?.path || section + '.md');
  }
  return ({worker_model:'Worker model',worker_reasoning:'Reasoning',worker_permissions:'Permissions',paused:'Paused',pause_reason:'Pause reason',prs_enabled:'Pull requests',worktree_enabled:'Worktrees',drafts_enabled:'Drafts',artifacts_enabled:'Artifacts'})[key] || key;
}
function configValue(value) {
  if (value === null || value === undefined) return 'Inherited default';
  if (value === '') return '(empty)';
  return typeof value === 'string' ? value : JSON.stringify(value, null, 2);
}
function configDiff(changes) {
  const container = configElement('div', undefined, 'config-diff');
  for (const [field, change] of Object.entries(changes || {})) {
    const section = configElement('section', undefined, 'config-diff-field');
    section.append(configElement('h4', configField(field)));
    const columns = configElement('div', undefined, 'config-diff-columns');
    for (const side of ['before', 'after']) {
      const column = configElement('div', undefined, 'config-diff-' + side);
      column.append(configElement('span', side === 'before' ? 'Before' : 'After', 'config-diff-label'), configElement('pre', configValue(change[side])));
      columns.append(column);
    }
    section.append(columns); container.append(section);
  }
  if (!container.children.length) container.append(configElement('p', 'No changes needed. These settings already match.', 'field-help'));
  return container;
}
async function loadConfigHistory(append = false) {
  if (!projectSettingsProject || configHistoryBusy) return;
  const sequence = ++configHistorySequence, project = projectSettingsProject;
  $('#config-history-status').textContent = 'Loading activity…';
  $('#config-history-refresh').disabled = true;
  $('#config-history-more').disabled = true;
  if (!append) { ++configDetailSequence; $('#config-history-list').replaceChildren(); $('#config-history-detail').hidden = true; }
  try {
    const value = await api({action:'project_config',operation:{command:'history',offset:append ? configHistoryOffset : 0}}, project);
    if (sequence !== configHistorySequence || project !== projectSettingsProject) return;
    $('#config-history-errors').replaceChildren();
    for (const copy of value.working_copies || []) if (copy.error) {
      $('#config-history-errors').append(configElement('p', copy.path + ': ' + copy.error, 'form-error'));
    }
    for (const revision of value.revisions) {
      const row = configElement('li', undefined, 'config-history-entry');
      const main = configElement('div', undefined, 'config-history-summary');
      const heading = configElement('div', undefined, 'config-history-heading');
      heading.append(configElement('strong', ({undo:'Undid a change',restore:'Restored a version',edit:'Updated configuration'})[revision.action] || revision.action));
      const time = configElement('time', new Date(revision.created_at).toLocaleString());
      time.dateTime = new Date(revision.created_at).toISOString(); heading.append(time);
      const author = revision.actor || {};
      main.append(heading, configElement('p', (author.id || 'Unknown editor') + ' · ' + (author.host || author.machine || 'Unknown machine') + ' · ' + (author.source || 'Unknown source'), 'config-history-author'));
      const fields = configElement('div', undefined, 'config-history-fields');
      for (const key of revision.fields) fields.append(configElement('span', configField(key)));
      main.append(fields);
      const button = configElement('button', 'View change', 'button'); button.type = 'button';
      button.onclick = () => showConfigRevision(revision, project);
      row.append(main, button); $('#config-history-list').append(row);
    }
    configHistoryOffset = value.next_offset;
    $('#config-history-more').hidden = configHistoryOffset === null;
    $('#config-history-status').textContent = $('#config-history-list').children.length ? '' : 'No changes recorded yet. New settings and prompt edits will appear here.';
  } catch (error) {
    if (sequence === configHistorySequence) $('#config-history-status').textContent = error.message;
  } finally {
    if (sequence === configHistorySequence) {
      $('#config-history-refresh').disabled = false; $('#config-history-more').disabled = false;
    }
  }
}
async function showConfigRevision(revision, project) {
  const sequence = ++configDetailSequence;
  const detail = $('#config-history-detail');
  detail.hidden = false; detail.replaceChildren(configElement('p', 'Loading change…'));
  try {
    const value = await api({action:'project_config',operation:{command:'view',revision:revision.id}}, project);
    if (sequence !== configDetailSequence || project !== projectSettingsProject) return;
    const render = (mode = 'view') => {
      const title = mode === 'view' ? 'What changed' : mode === 'undo' ? 'Review undo' : 'Review restore';
      const heading = configElement('div', undefined, 'config-activity-heading');
      heading.append(configElement('h3', title));
      const close = configElement('button', 'Close change', 'button'); close.type = 'button';
      close.onclick = () => { detail.hidden = true; }; heading.append(close);
      detail.replaceChildren(heading, configElement('p', mode === 'undo' ? 'Only the fields in this change will be reversed. Unrelated later edits stay.' : mode === 'restore' ? 'All settings and prompts will return to this version. This creates a new history entry.' : revision.id, 'field-help'));
      const changes = value[mode === 'view' ? 'changes' : mode + '_changes'];
      detail.append(configDiff(changes));
      const actions = configElement('div', undefined, 'config-history-actions');
      for (const command of mode === 'view' ? ['undo', 'restore'] : [mode]) {
        const button = configElement('button', mode === 'view' ? command === 'undo' ? 'Undo this change…' : 'Restore this version…' : command === 'undo' ? 'Undo these changes' : 'Restore these settings', 'button');
        button.type = 'button';
        button.disabled = mode !== 'view' && !Object.keys(changes).length;
        button.onclick = async () => {
          if (mode === 'view') { render(command); return; }
          if (JSON.stringify(projectSettingsDraft()) !== JSON.stringify(projectSettingsOriginal)) {
            $('#config-history-status').textContent = 'Save or discard your unsaved settings before undoing or restoring.'; return;
          }
          configHistoryBusy = true; projectSettingsSaving = true;
          for (const control of detail.querySelectorAll('button')) control.disabled = true;
          setProjectSettingsDisabled(true);
          try {
            await mutate({action:'project_config',operation:{command,revision:revision.id,if_digest:value.digest}}, project);
            projectSettingsSaving = false; configHistoryBusy = false;
            await $('#project-settings-trigger').onclick();
            selectProjectSettingsTab($('#settings-tab-activity'));
            detailCache.clear(); await refreshProjects(project); updateHeader();
            toast(command === 'undo' ? 'Change undone. History retained.' : 'Version restored. History retained.');
          } catch (error) {
            detail.append(configElement('p', error.message, 'form-error'));
          } finally {
            configHistoryBusy = false; projectSettingsSaving = false;
            setProjectSettingsDisabled(false); projectSettingsChanged();
            for (const control of detail.querySelectorAll('button')) control.disabled = false;
          }
        };
        actions.append(button);
      }
      if (mode !== 'view') {
        const back = configElement('button', 'Back', 'button'); back.type = 'button'; back.onclick = () => render(); actions.append(back);
      }
      detail.append(actions); detail.scrollIntoView({block:'nearest'});
    };
    render();
  } catch (error) {
    if (sequence === configDetailSequence) detail.replaceChildren(configElement('p', error.message, 'form-error'));
  }
}
$('#config-history-refresh').onclick = () => loadConfigHistory();
$('#config-history-more').onclick = () => loadConfigHistory(true);
window.addEventListener('resize', () => {
  if ($('#project-settings-dialog').open) $('.project-settings-tabs [aria-selected="true"]').scrollIntoView({block:'nearest',inline:'nearest'});
});
