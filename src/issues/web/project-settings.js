"use strict";
const workflowPromptKeys = ["plan", "worktree", "checkout", "prs", "main"];
let projectPromptDefaults = {};
let chiefDefaultPrompt = "";
let projectSettingsVersion = 0,
  projectSettingsProject = null,
  projectSettingsFocus = null,
  projectSettingsOriginal = null,
  projectSettingsSaving = false,
  projectPreviewFailed = false,
  projectPreviewTimer,
  projectPreviewSequence = 0,
  projectSettingsSequence = 0;
function projectSettingsDraft() {
  return {
    subtask_scheduling: $("#project-subtask-scheduling").value,
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
  $("#project-scheduling-help").textContent = $("#project-subtask-scheduling").value === "explicit"
    ? "Independent siblings can run in parallel. Use Add blocker on an issue to declare dependencies and intentional sequences. Queue order sets priority only."
    : "Each sibling and its descendants wait for all earlier siblings to finish. Queue order sets the sequence.";
  $("#project-chief-state").textContent = $("#project-chief").checked ? "Enabled" : "Disabled";
  $(".chief-settings").classList.toggle("chief-enabled", $("#project-chief").checked);
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
  $("#project-settings-error").hidden = true;
  projectPreviewFailed = false;
  $("#project-settings-state").textContent = "Loading…";
  setProjectSettingsDisabled(true);
  $("#project-instructions-preview").textContent = "Loading…";
  $("#project-goal-indicator").hidden = true;
  $("#project-settings-form button[type=submit]").disabled = true;
  $("#project-settings-dialog").showModal();
  try {
    const value = await api(
      { action: "project_settings" },
      projectSettingsProject,
    );
    if (sequence !== projectSettingsSequence) return;
    projectSettingsVersion = value.version;
    $("#project-subtask-scheduling").value = value.subtask_scheduling || "sequential";
    $("#project-prompt").value = value.prompt;
    $("#project-chief").checked = value.chief_enabled;
    $("#project-chief-prompt").value = value.chief_prompt;
    chiefDefaultPrompt = value.chief_default_prompt;
    $("#project-chief-instructions").open = false;
    $("#project-prs").checked = value.prs_enabled;
    $("#project-worktree").checked = value.worktree_enabled;
    $("#project-preview-workspace").value = "checkout";
    $("#project-preview-task").value = "implement";
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
      const value = await api(
        {
          action: "preview_worker",
          task_kind: $("#project-preview-task").value,
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
$("#project-subtask-scheduling").onchange = projectSettingsChanged;
$("#project-plan-template").oninput = projectSettingsChanged;
$("#project-chief").onchange = projectSettingsChanged;
$("#project-chief-prompt").oninput = projectSettingsChanged;
$("#project-chief-reset").onclick = () => {
  $("#project-chief-prompt").value = chiefDefaultPrompt;
  projectSettingsChanged();
};

function setProjectSettingsDisabled(disabled) {
  $("#project-settings-close").disabled = projectSettingsSaving;
  $("#project-settings-cancel").disabled = projectSettingsSaving;
  for (const input of document.querySelectorAll("#project-settings-form input, #project-settings-form textarea, #project-subtask-scheduling, [data-reset-prompt], #project-chief-reset")) input.disabled = disabled;
  $("#project-preview-workspace").disabled = disabled;
  $("#project-preview-task").disabled = disabled;
}
function updateWorkflowBranches() {
  const plan = $("#project-preview-task").value === "plan";
  const allowed = $("#project-worktree").checked;
  $("#project-preview-workspace option[value=worktree]").disabled = !allowed;
  if (!allowed) $("#project-preview-workspace").value = "checkout";
  const active = plan ? ["plan"] : [$("#project-preview-workspace").value, $("#project-prs").checked ? "prs" : "main"];
  $("#project-preview-workspace").disabled = plan || $("#project-preview-task").disabled;
  $("#project-preview-workspace-help").textContent = plan ? "Plan tasks use their own prompt without workspace or delivery instructions." : "Preview only. Each worker chooses its own workspace.";
  for (const key of workflowPromptKeys) {
    const branch = document.querySelector(`[data-branch="${key}"]`);
    branch.classList.toggle("active", active.includes(key));
    branch.querySelector(".branch-state").textContent = active.includes(key) ? "In preview" : key === "worktree" ? (allowed ? "Available" : "Not allowed") : key === "checkout" ? "Available" : "Inactive";
    const custom = !!$("#project-prompt-" + key).value.trim();
    $("#project-source-" + key).textContent = custom ? "Project override" : "Built-in default";
    branch.querySelector("[data-reset-prompt]").hidden = !custom;
  }
  $("#project-preview-choices").textContent = plan ? "Plan · Linked artifacts" : `${active[0] === "worktree" ? "Dedicated worktree" : "Existing checkout"} · ${active[1] === "prs" ? "Pull requests" : "Push to main"}`;
}
$("#project-preview-task").onchange = () => {
  updateWorkflowBranches();
  previewProjectInstructions();
};
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
