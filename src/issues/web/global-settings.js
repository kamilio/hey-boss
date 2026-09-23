"use strict";
let globalSettingsVersion = null,
  globalSettingsOriginal = null,
  globalAutoCloseOriginal = true,
  globalSettingsSaving = false,
  globalSettingsSequence = 0,
  globalSettingsHost = null,
  globalSettingsPending = null;
function updateProfile() {
  const name = model.boss.name;
  $("#self-avatar").textContent = initials(name);
  $("#self-avatar").title = `${name} · human:boss`;
  $("#self-avatar").setAttribute("aria-label", `Profile: ${name}`);
  $("#profile-name").textContent = name;
}
function initGlobalSettings() {
  function closeProfile(focus = false) {
    $("#profile-menu").hidden = true;
    $("#self-avatar").setAttribute("aria-expanded", "false");
    if (focus) $("#self-avatar").focus();
  }
  function openProfile() {
    closeProjectMenu();
    $("#profile-menu").hidden = false;
    $("#self-avatar").setAttribute("aria-expanded", "true");
    $("#global-settings-trigger").focus();
  }
  $("#self-avatar").onclick = () =>
    $("#profile-menu").hidden ? openProfile() : closeProfile(true);
  $("#self-avatar").onkeydown = (event) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      openProfile();
    }
  };
  $("#profile-menu").onkeydown = (event) => {
    if (["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
      event.preventDefault();
      $("#global-settings-trigger").focus();
    }
  };
  document.addEventListener("click", (event) => {
    if (!event.target.closest(".profile-control")) closeProfile();
  });
  document.addEventListener("keydown", (event) => {
    if (!$("#profile-menu").hidden && event.key === "Escape") {
      event.preventDefault();
      closeProfile(true);
    }
    if (!$("#profile-menu").hidden && event.key === "Tab") closeProfile(true);
  });
  function globalSettingsChanged() {
    const changed =
      globalSettingsOriginal !== null &&
      ($("#global-boss-name").value.trim() !== globalSettingsOriginal || $("#global-auto-close-prs").checked !== globalAutoCloseOriginal);
    $("#global-settings-submit").disabled = globalSettingsSaving || !changed;
    $("#global-settings-state").textContent = changed ? "Unsaved changes" : "";
  }
  function closeGlobalSettings() {
    if (globalSettingsSaving) return;
    ++globalSettingsSequence;
    $("#global-settings-dialog").close();
    $("#self-avatar").focus();
  }
  $("#global-settings-trigger").onclick = async () => {
    closeProfile();
    const sequence = ++globalSettingsSequence;
    globalSettingsOriginal = null;
    globalSettingsVersion = null;
    globalSettingsHost = model.route.host || null;
    $("#global-boss-name").value = model.boss.name;
    $("#global-boss-name").disabled = true;
    $("#global-auto-close-prs").disabled = true;
    $("#global-settings-submit").disabled = true;
    $("#global-settings-error").hidden = true;
    $("#global-settings-reload").hidden = true;
    $("#global-settings-state").textContent = "Loading…";
    $("#global-settings-dialog").showModal();
    try {
      const value = await api(
        { action: "global_settings" },
        model.project.id,
        null,
        globalSettingsHost,
      );
      if (sequence !== globalSettingsSequence) return;
      globalSettingsVersion = value.version;
      globalSettingsOriginal = value.boss_name;
      globalAutoCloseOriginal = value.auto_close_merged_prs ?? true;
      $("#global-boss-name").value = value.boss_name;
      $("#global-auto-close-prs").checked = globalAutoCloseOriginal;
      $("#global-boss-name").disabled = false;
      $("#global-auto-close-prs").disabled = false;
      globalSettingsChanged();
      $("#global-boss-name").focus();
    } catch (error) {
      if (sequence !== globalSettingsSequence) return;
      $("#global-settings-state").textContent = "";
      $("#global-settings-error").textContent = error.message;
      $("#global-settings-error").hidden = false;
    }
  };
  $("#global-auto-close-prs").onchange = globalSettingsChanged;
  $("#global-boss-name").oninput = globalSettingsChanged;
  for (const selector of ["#global-settings-close", "#global-settings-cancel"])
    $(selector).onclick = closeGlobalSettings;
  $("#global-settings-dialog").addEventListener("cancel", (event) => {
    event.preventDefault();
    closeGlobalSettings();
  });

  $("#global-settings-reload").onclick = async () => {
    if (globalSettingsSaving) return;
    const sequence = globalSettingsSequence;
    $("#global-settings-reload").disabled = true;
    try {
      const value = await api(
        { action: "global_settings" },
        model.project.id,
        null,
        globalSettingsHost,
      );
      if (sequence !== globalSettingsSequence) return;
      globalSettingsVersion = value.version;
      globalSettingsOriginal = value.boss_name;
      globalAutoCloseOriginal = value.auto_close_merged_prs ?? true;
      globalSettingsPending = null;
      $("#global-settings-error").textContent =
        `Current name: ${value.boss_name}. Your draft is preserved.`;
      $("#global-settings-reload").hidden = true;
      globalSettingsChanged();
    } catch (error) {
      $("#global-settings-error").textContent = error.message;
    } finally {
      $("#global-settings-reload").disabled = false;
    }
  };
  $("#global-settings-form").onsubmit = async (event) => {
    event.preventDefault();
    if (globalSettingsSaving || globalSettingsVersion === null) return;
    globalSettingsSaving = true;
    $("#global-boss-name").disabled = true;
    $("#global-auto-close-prs").disabled = true;
    $("#global-settings-submit").disabled = true;
    $("#global-settings-state").textContent = "Saving…";
    try {
      const operation = {
        action: "configure_global",
        auto_close_merged_prs: $("#global-auto-close-prs").checked,
        boss_name: $("#global-boss-name").value.trim(),
        if_version: globalSettingsVersion,
      };
      const key = JSON.stringify([globalSettingsHost, operation]);
      if (globalSettingsPending?.key !== key)
        globalSettingsPending = { key, id: crypto.randomUUID() };
      const value = await api(
        operation,
        model.project.id,
        globalSettingsPending.id,
        globalSettingsHost,
      );
      globalSettingsPending = null;
      globalSettingsVersion = value.version;
      globalSettingsOriginal = value.boss_name;
      globalAutoCloseOriginal = value.auto_close_merged_prs ?? true;
      detailCache.clear();
      globalSettingsSaving = false;
      closeGlobalSettings();
      await renderRoute();
      toast("Settings saved");
    } catch (error) {
      $("#global-settings-state").textContent = "";
      $("#global-settings-error").textContent = error.message;
      $("#global-settings-error").hidden = false;
      $("#global-settings-reload").hidden = error.code !== "conflict";
    } finally {
      globalSettingsSaving = false;
      $("#global-boss-name").disabled = false;
      $("#global-auto-close-prs").disabled = false;
      globalSettingsChanged();
    }
  };
}
