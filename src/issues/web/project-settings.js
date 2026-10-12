"use strict";
const workflowPromptKeys = ["plan", "worktree", "checkout", "prs", "main"];
let projectPromptDefaults = {};
let projectPromptLayout = null;
function addPromptEditors(sections) {
  workflowPromptKeys.splice(5);
  for (const id of ["context", "runtime"]) $("#project-" + id + "-prompts").replaceChildren();
  for (const section of sections || []) {
    const key = section.key;
    if (key === "layout" || workflowPromptKeys.includes(key)) continue;
    workflowPromptKeys.push(key);
    const group = ["steering", "dependency_update", "prompt_update", "goal_continue"].includes(key) ? "runtime" : "context";
    const branch = document.createElement("section");
    branch.className = "workflow-branch settings-section";
    branch.dataset.branch = key;
    const heading = document.createElement("div"); heading.className = "branch-heading";
    const label = document.createElement("label"); label.htmlFor = "project-prompt-" + key;
    const title = document.createElement("strong"); title.textContent = section.title; label.append(title);
    const state = document.createElement("span"); state.className = "branch-state";
    heading.append(label, state);
    const help = document.createElement("p"); help.className = "field-help"; help.textContent = section.help;
    const input = document.createElement("textarea"); input.className = "text-input workflow-prompt";
    input.id = "project-prompt-" + key; input.rows = 4; input.maxLength = 32000;
    input.setAttribute("aria-describedby", "project-source-" + key);
    input.addEventListener("input", () => { projectSettingsChanged(); previewProjectInstructions(); });
    const footer = document.createElement("div"); footer.className = "branch-footer";
    const source = document.createElement("span"); source.id = "project-source-" + key;
    const reset = document.createElement("button"); reset.type = "button"; reset.className = "button link-button";
    reset.dataset.resetPrompt = key; reset.textContent = "Use default";
    reset.onclick = () => { input.value = ""; projectSettingsChanged(); previewProjectInstructions(); };
    footer.append(source, reset);
    branch.append(heading, help, input, footer);
    $("#project-" + group + "-prompts").append(branch);
  }
}
let projectSettingsTab = "configuration";
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
  $(".project-settings-body").dataset.settingsTab = projectSettingsTab;
  $(".settings-preview").hidden = projectSettingsTab === "configuration";
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
    worktree_enabled: $("#project-worktree").checked,
    // The sequence has no editor; retain saved layouts in previews and unrelated edits.
    prompt_overrides: {layout: projectPromptLayout, ...Object.fromEntries(workflowPromptKeys.map(key => [key, $("#project-prompt-" + key).value.trim() ? $("#project-prompt-" + key).value : null]))},
    prs_enabled: $("#project-prs").checked,
    drafts_enabled: $("#project-drafts").checked,
    artifacts_enabled: $("#project-artifacts").checked,
    mindmaps_enabled: $("#project-mindmaps").checked,
  };
}
function projectSettingsChanged() {
  const changed =
    JSON.stringify(projectSettingsDraft()) !==
    JSON.stringify(projectSettingsOriginal);
  updateWorkflowBranches();
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
  $("#project-settings-name").textContent = model.project.name;
  $("#project-settings-name").title = model.project.name;
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
    $("#project-prs").checked = value.prs_enabled;
    $("#project-worktree").checked = value.worktree_enabled;
    $("#project-preview-workspace").value = "checkout";
    projectPromptDefaults = value.prompt_defaults;
    projectPromptLayout = value.prompt_overrides.layout ?? null;
    addPromptEditors(value.prompt_sections);
    for (const key of workflowPromptKeys) {
      const input = $("#project-prompt-" + key);
      input.value = value.prompt_overrides[key] ?? "";
      input.placeholder = projectPromptDefaults[key];
    }
    for (const resource of ["drafts", "artifacts", "mindmaps"]) $("#project-" + resource).checked = value[resource + "_enabled"] !== false;
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
  if (projectSettingsTab === "configuration") {
    $("#project-instructions-preview").setAttribute("aria-busy", "false");
    return;
  }
  $("#project-instructions-preview").setAttribute("aria-busy", "true");
  projectPreviewTimer = setTimeout(async () => {
    try {
      const value = await api(
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

for (const resource of ["drafts", "artifacts", "mindmaps"]) $("#project-" + resource).onchange = projectSettingsChanged;
function setProjectSettingsDisabled(disabled) {
  $("#project-settings-close").disabled = projectSettingsSaving;
  $("#project-settings-cancel").disabled = projectSettingsSaving;
  for (const input of document.querySelectorAll("#project-settings-form input, #project-settings-form textarea, [data-reset-prompt]")) input.disabled = disabled;
  $("#project-preview-workspace").disabled = disabled;
}
function updateWorkflowBranches() {
  const plan = projectSettingsTab === "planning";
  $("#project-preview-label").textContent = plan ? "PLAN PREVIEW" : "IMPLEMENTATION PREVIEW";
  $("#project-preview-help").textContent = plan ? "Plan instructions. Updates as you edit." : "Includes workspace and delivery instructions. Updates as you edit.";
  $("#project-preview-worker").hidden = plan;
  const allowed = $("#project-worktree").checked;
  $("#project-preview-workspace option[value=worktree]").disabled = !allowed;
  if (!allowed) $("#project-preview-workspace").value = "checkout";
  const active = plan ? ["plan"] : [$("#project-preview-workspace").value, $("#project-prs").checked ? "prs" : "main"];
  $("#project-preview-workspace").disabled = plan || projectSettingsSaving || !projectSettingsOriginal;
  $("#project-preview-workspace-help").textContent = plan ? "Plan tasks use their own prompt without workspace or delivery instructions." : "Preview only. Each worker chooses its own workspace.";
  for (const key of workflowPromptKeys) {
    const branch = document.querySelector(`[data-branch="${key}"]`);
    branch.classList.toggle("active", active.includes(key));
    branch.querySelector(".branch-state").textContent = active.includes(key) ? "In preview" : key === "worktree" ? (allowed ? "Available" : "Not allowed") : key === "checkout" ? "Available" : workflowPromptKeys.indexOf(key) >= 5 ? "When applicable" : "Inactive";
    const custom = !!$("#project-prompt-" + key).value.trim();
    $("#project-source-" + key).textContent = custom ? "Project override" : "Built-in default";
    branch.querySelector("[data-reset-prompt]").hidden = !custom;
  }
  $("#project-preview-choices").textContent = plan ? "Plan · Linked artifacts" : `${active[0] === "worktree" ? "Dedicated worktree" : "Existing checkout"} · ${active[1] === "prs" ? "Pull requests" : "Push to main"}`;
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
