"use strict";
let projectSettingsVersion = 0,
  projectSettingsProject = null,
  projectSettingsFocus = null,
  projectSettingsOriginal = null,
  projectSettingsSaving = false,
  projectPreviewTimer,
  projectPreviewSequence = 0,
  projectSettingsSequence = 0;
function projectSettingsDraft() {
  return {
    prompt: $("#project-prompt").value,
    prs_enabled: $("#project-prs").checked,
    drafts_enabled: $("#project-drafts").checked,
    plan_template: $("#project-plan-template").value,
  };
}
function projectSettingsChanged() {
  const changed =
    JSON.stringify(projectSettingsDraft()) !==
    JSON.stringify(projectSettingsOriginal);
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
  $("#project-settings-state").textContent = "Loading…";
  $("#project-prompt").disabled = true;
  $("#project-prs").disabled = true;
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
    $("#project-prompt").value = value.prompt;
    $("#project-prs").checked = value.prs_enabled;
    $("#project-drafts").checked = value.drafts_enabled;
    $("#project-plan-template").value = value.plan_template;
    projectSettingsOriginal = projectSettingsDraft();
    $("#project-prompt").disabled = false;
    $("#project-prs").disabled = false;
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
  projectSettingsChanged();
  $("#project-settings-state").textContent = "Saving…";
  try {
    const draft = projectSettingsDraft();
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
    toast("Settings saved");
  } catch (error) {
    $("#project-settings-error").textContent = error.message;
    $("#project-settings-error").hidden = false;
  } finally {
    projectSettingsSaving = false;
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
          config: {
            projects: [project],
            prompt: draft.prompt,
            prs_enabled: draft.prs_enabled,
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
    } catch (error) {
      if (
        sequence !== projectPreviewSequence ||
        !$("#project-settings-dialog").open
      )
        return;
      $("#project-settings-error").textContent = error.message;
      $("#project-settings-error").hidden = false;
    } finally {
      if (sequence === projectPreviewSequence)
        $("#project-instructions-preview").setAttribute("aria-busy", "false");
    }
  }, 150);
}
for (const selector of ["#project-prompt", "#project-prs"]) {
  $(selector).addEventListener("input", () => {
    projectSettingsChanged();
    previewProjectInstructions();
  });
}

$("#project-drafts").onchange = projectSettingsChanged;
$("#project-plan-template").oninput = projectSettingsChanged;
