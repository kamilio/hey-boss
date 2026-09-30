"use strict";
const workflowPromptKeys = ["plan", "worktree", "checkout", "prs", "main"];
let projectPromptDefaults = {};
let chiefDefaultPrompt = "", chiefPreviewTemplate = "", projectSettingsTab = "instructions";
let chiefRunPending = false, chiefRunRequest = null;
let projectSettingsVersion = 0,
  projectSettingsProject = null,
  projectSettingsFocus = null,
  projectSettingsOriginal = null,
  projectSettingsSaving = false,
  projectPreviewFailed = false,
  projectPreviewTimer,
  projectPreviewSequence = 0,
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
  $(".settings-preview").scrollTop = 0;
  projectSettingsTab = tab.id.replace('settings-tab-', '');
  $(".project-settings-body").scrollTop = 0;
  updateWorkflowBranches();
  if (projectSettingsOriginal) previewProjectInstructions();
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
    prompt: $("#project-prompt").value,
    chief_enabled: $("#project-chief").checked,
    chief_prompt: $("#project-chief-prompt").value,
    worktree_enabled: $("#project-worktree").checked,
    prompt_overrides: Object.fromEntries(workflowPromptKeys.map(key => [key, $("#project-prompt-" + key).value.trim() ? $("#project-prompt-" + key).value : null])),
    prs_enabled: $("#project-prs").checked,
    drafts_enabled: $("#project-drafts").checked,
    plan_template: $("#project-plan-template").value,
  };
}
function projectSettingsChanged() {
  const changed =
    JSON.stringify(projectSettingsDraft()) !==
    JSON.stringify(projectSettingsOriginal);
  updateWorkflowBranches();
  $("#project-chief-state").textContent = $("#project-chief").checked ? "Enabled" : "Disabled";
  $(".chief-settings").classList.toggle("chief-enabled", $("#project-chief").checked);
  $("#project-chief-run").disabled = chiefRunPending || projectSettingsSaving || !projectSettingsOriginal?.chief_enabled || changed;
  $("#project-settings-state").textContent = changed ? "Unsaved changes" : "";
  $("#project-settings-form button[type=submit]").disabled =
    projectSettingsSaving || !projectSettingsOriginal || !changed;
}
function closeProjectSettings() {
  if (projectSettingsSaving) return;
  clearTimeout(projectPreviewTimer);
  ++projectPreviewSequence;
  ++projectSettingsSequence;
  $("#project-settings-dialog").close();
  projectSettingsFocus?.focus();
}
$("#project-settings-trigger").onclick = async () => {
  const sequence = ++projectSettingsSequence;
  projectSettingsProject = model.project.id;
  projectSettingsFocus = $("#project-settings-trigger");
  projectSettingsOriginal = null;
  chiefRunRequest = null;
  $("#project-chief-actions").open = false;
  $("#project-chief-run-status").textContent = "";
  $("#project-settings-name").textContent = model.project.name;
  $("#project-settings-error").hidden = true;
  projectPreviewFailed = false;
  $("#project-settings-state").textContent = "Loading…";
  setProjectSettingsDisabled(true);
  $("#project-instructions-preview").textContent = "Loading…";
  $("#project-goal-indicator").hidden = true;
  $("#project-settings-form button[type=submit]").disabled = true;
  $("#project-settings-dialog").showModal();
  selectProjectSettingsTab(projectSettingsTabs[0]);
  try {
    const value = await api(
      { action: "project_settings" },
      projectSettingsProject,
    );
    if (sequence !== projectSettingsSequence) return;
    projectSettingsVersion = value.version;
    $("#project-prompt").value = value.prompt;
    $("#project-chief").checked = value.chief_enabled;
    $("#project-chief-prompt").value = value.chief_prompt;
    chiefDefaultPrompt = value.chief_default_prompt;
    chiefPreviewTemplate = value.chief_preview_template;
    $("#project-chief-instructions").open = true;
    $("#project-prs").checked = value.prs_enabled;
    $("#project-worktree").checked = value.worktree_enabled;
    $("#project-preview-workspace").value = "checkout";
    projectPromptDefaults = value.prompt_defaults;
    for (const key of workflowPromptKeys) {
      const input = $("#project-prompt-" + key);
      input.value = value.prompt_overrides[key] ?? "";
      input.placeholder = projectPromptDefaults[key];
    }
    $("#project-drafts").checked = value.drafts_enabled;
    $("#project-plan-template").value = value.plan_template;
    projectSettingsOriginal = projectSettingsDraft();
    setProjectSettingsDisabled(false);
    projectSettingsChanged();
    previewProjectInstructions();
  } catch (error) {
    if (sequence !== projectSettingsSequence) return;
    $("#project-settings-state").textContent = "";
    $("#project-settings-error").textContent = error.message;
    $("#project-settings-error").hidden = false;
  }
};
$("#project-settings-close").onclick = closeProjectSettings;
$("#project-chief-run").onclick = async () => {
  if (chiefRunPending || $("#project-chief-run").disabled) return;
  const project = projectSettingsProject, sequence = projectSettingsSequence;
  chiefRunPending = true;
  chiefRunRequest ||= {project, id: HeyBossUI.requestId()};
  projectSettingsChanged();
  $("#project-chief-run-status").textContent = "Queuing Chief…";
  try {
    await post("/api/fleet/chief", chiefRunRequest);
    if (sequence !== projectSettingsSequence) return;
    chiefRunRequest = null;
    $("#project-chief-run-status").textContent = "Chief queued. If a pass is running, one fresh pass will follow it.";
  } catch (error) {
    if (sequence === projectSettingsSequence) $("#project-chief-run-status").textContent = error.message;
  } finally {
    chiefRunPending = false;
    projectSettingsChanged();
  }
};
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
    const promptsChanged = draft.prompt !== projectSettingsOriginal.prompt ||
      JSON.stringify(draft.prompt_overrides) !== JSON.stringify(projectSettingsOriginal.prompt_overrides);
    const value = await mutate(
      {
        action: "configure_project",
        ...draft,
        if_version: projectSettingsVersion,
      },
      projectSettingsProject,
    );
    projectSettingsVersion = value.version;
    projectSettingsOriginal = draft;
    $("#project-settings-error").hidden = true;
    projectSettingsSaving = false;
    closeProjectSettings();
    detailCache.clear();
    await renderRoute();
    updateHeader();
    toast(promptsChanged ? "Settings saved. Instruction updates queued for running agents." : "Settings saved");
  } catch (error) {
    projectPreviewFailed = false;
    $("#project-settings-error").textContent = error.message;
    $("#project-settings-error").hidden = false;
  } finally {
    projectSettingsSaving = false;
    if (projectSettingsOriginal) setProjectSettingsDisabled(false);
    projectSettingsChanged();
  }
};
function previewProjectInstructions() {
  clearTimeout(projectPreviewTimer);
  const sequence = ++projectPreviewSequence,
    project = projectSettingsProject,
    draft = projectSettingsDraft();
  $("#project-instructions-preview").setAttribute("aria-busy", "true");
  projectPreviewTimer = setTimeout(async () => {
    try {
      const value = projectSettingsTab === "chief" ? {
        prompt: chiefPreviewTemplate.replace(/{{(project|prompt)}}/g, (_, key) => key === "project" ? project : draft.chief_prompt),
        use_goal: false,
      } : await api(
        {
          action: "preview_worker",
          task_kind: projectSettingsTab === "planning" ? "plan" : "implement",
          worktree_allowed: draft.worktree_enabled,
          config: {
            projects: [project],
            prompt: draft.prompt,
            prs_enabled: draft.prs_enabled,
            worktree_enabled: draft.worktree_enabled && $("#project-preview-workspace").value === "worktree",
            prompt_overrides: draft.prompt_overrides,
          },
          number: null,
        },
        project,
      );
      if (
        sequence !== projectPreviewSequence ||
        !$("#project-settings-dialog").open ||
        project !== projectSettingsProject
      )
        return;
      $("#project-instructions-preview").textContent = value.prompt;
      $("#project-goal-indicator").hidden = !value.use_goal;
      if (projectPreviewFailed) {
        $("#project-settings-error").hidden = true;
        projectPreviewFailed = false;
      }
    } catch (error) {
      if (
        sequence !== projectPreviewSequence ||
        !$("#project-settings-dialog").open
      )
        return;
      if ($("#project-settings-error").hidden || projectPreviewFailed) {
        $("#project-settings-error").textContent = error.message;
        $("#project-settings-error").hidden = false;
        projectPreviewFailed = true;
      }
    } finally {
      if (sequence === projectPreviewSequence)
        $("#project-instructions-preview").setAttribute("aria-busy", "false");
    }
  }, 150);
}
for (const selector of ["#project-prompt", "#project-prs", "#project-worktree", ...workflowPromptKeys.map(key => "#project-prompt-" + key)]) {
  $(selector).addEventListener("input", () => {
    projectSettingsChanged();
    previewProjectInstructions();
  });
}

$("#project-drafts").onchange = projectSettingsChanged;
$("#project-plan-template").oninput = projectSettingsChanged;
$("#project-chief").onchange = projectSettingsChanged;
$("#project-chief-prompt").oninput = () => { projectSettingsChanged(); previewProjectInstructions(); };
$("#project-chief-reset").onclick = () => {
  $("#project-chief-prompt").value = chiefDefaultPrompt;
  projectSettingsChanged();
  previewProjectInstructions();
};

function setProjectSettingsDisabled(disabled) {
  $("#project-chief-run").disabled = disabled || chiefRunPending || !projectSettingsOriginal?.chief_enabled;
  $("#project-settings-close").disabled = projectSettingsSaving;
  $("#project-settings-cancel").disabled = projectSettingsSaving;
  for (const input of document.querySelectorAll("#project-settings-form input, #project-settings-form textarea, [data-reset-prompt], #project-chief-reset")) input.disabled = disabled;
  $("#project-preview-workspace").disabled = disabled;
}
function updateWorkflowBranches() {
  const plan = projectSettingsTab === "planning", chief = projectSettingsTab === "chief";
  $("#project-preview-label").textContent = chief ? "CHIEF PREVIEW" : plan ? "PLAN PREVIEW" : "IMPLEMENTATION PREVIEW";
  $("#project-preview-help").textContent = chief ? "Chief instructions. Updates as you edit." : plan ? "Plan instructions. Updates as you edit." : "Includes workspace and delivery instructions. Updates as you edit.";
  $("#project-preview-worker").hidden = plan || chief;
  const allowed = $("#project-worktree").checked;
  $("#project-preview-workspace option[value=worktree]").disabled = !allowed;
  if (!allowed) $("#project-preview-workspace").value = "checkout";
  const active = chief ? [] : plan ? ["plan"] : [$("#project-preview-workspace").value, $("#project-prs").checked ? "prs" : "main"];
  $("#project-preview-workspace").disabled = plan || chief || projectSettingsSaving || !projectSettingsOriginal;
  $("#project-preview-workspace-help").textContent = plan ? "Plan tasks use their own prompt without workspace or delivery instructions." : "Preview only. Each worker chooses its own workspace.";
  for (const key of workflowPromptKeys) {
    const branch = document.querySelector(`[data-branch="${key}"]`);
    branch.classList.toggle("active", active.includes(key));
    branch.querySelector(".branch-state").textContent = active.includes(key) ? "In preview" : key === "worktree" ? (allowed ? "Available" : "Not allowed") : key === "checkout" ? "Available" : "Inactive";
    const custom = !!$("#project-prompt-" + key).value.trim();
    $("#project-source-" + key).textContent = custom ? "Project override" : "Built-in default";
    branch.querySelector("[data-reset-prompt]").hidden = !custom;
  }
  $("#project-preview-choices").textContent = chief ? "Chief · Organizing pass" : plan ? "Plan · Linked artifacts" : `${active[0] === "worktree" ? "Dedicated worktree" : "Existing checkout"} · ${active[1] === "prs" ? "Pull requests" : "Push to main"}`;
}
$("#project-preview-workspace").onchange = () => {
  updateWorkflowBranches();
  previewProjectInstructions();
};
for (const button of document.querySelectorAll("[data-reset-prompt]")) {
  button.onclick = () => {
    $("#project-prompt-" + button.dataset.resetPrompt).value = "";
    projectSettingsChanged();
    previewProjectInstructions();
  };
}
