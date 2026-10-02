"use strict";
let globalSettingsVersion = null,
  globalSettingsOriginal = null,
  globalAutoCloseOriginal = true,
  globalQuietOriginal = null,
  globalSettingsSaving = false,
  globalSettingsSequence = 0,
  globalSettingsHost = null,
  globalSettingsPending = null;
function updateProfile() {
  if (new URLSearchParams(location.hash.slice(1)).get("settings") === "1") {
    history.replaceState(null, "", location.pathname + location.search);
    setTimeout(() => $("#global-settings-trigger").click(), 0);
  }
  const name = model.boss.name;
  $("#self-avatar").textContent = initials(name);
  $("#self-avatar").title = `${name} · human:boss`;
  $("#self-avatar").setAttribute("aria-label", `Profile: ${name}`);
  $("#profile-name").textContent = name;
}
function initGlobalSettings() {
  const quiet = () => ({enabled: $("#global-quiet-enabled").checked, start: $("#global-quiet-start").value, end: $("#global-quiet-end").value, time_zone: $("#global-quiet-zone").value});
  const loadQuiet = (value) => {
    globalQuietOriginal = value.quiet_hours || {enabled: true, start: "22:00", end: "07:00", time_zone: Intl.DateTimeFormat().resolvedOptions().timeZone};
    const q = globalQuietOriginal;
    $("#global-quiet-enabled").checked = q.enabled;
    $("#global-quiet-start").value = q.start;
    $("#global-quiet-end").value = q.end;
    const zones = [...new Set([q.time_zone, "UTC", ...(Intl.supportedValuesOf?.("timeZone") || [])])].sort();
    $("#global-quiet-zone").replaceChildren(...zones.map(zone => new Option(zone.replaceAll("_", " "), zone)));
    $("#global-quiet-zone").value = q.time_zone;
  };
  const disableQuiet = (disabled) => {
    $("#global-quiet-enabled").disabled = disabled;
    $("#global-quiet-fields").disabled = disabled || !$("#global-quiet-enabled").checked;
  };
  for (const id of ["enabled", "start", "end", "zone"]) $("#global-quiet-" + id).onchange = globalSettingsChanged;

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
    disableQuiet(globalSettingsSaving || globalSettingsOriginal === null);
    const q = quiet();
    $("#global-quiet-end").setCustomValidity(q.start === q.end ? "Choose different start and end times." : "");
    $("#global-quiet-summary").textContent = q.enabled ? `Every day, ${q.start}–${q.end}${q.start > q.end ? " the next morning" : ""}. Applies across your devices.` : "Notifications follow your usual delivery settings.";
    const changed =
      globalSettingsOriginal !== null &&
      ($("#global-boss-name").value.trim() !== globalSettingsOriginal ||
       $("#global-auto-close-prs").checked !== globalAutoCloseOriginal ||
       Object.keys(q).some(key => q[key] !== globalQuietOriginal?.[key]));
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
    disableQuiet(true);
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
      loadQuiet(value);
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
      globalQuietOriginal = value.quiet_hours;
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
    disableQuiet(true);
    $("#global-boss-name").disabled = true;
    $("#global-auto-close-prs").disabled = true;
    $("#global-settings-submit").disabled = true;
    $("#global-settings-state").textContent = "Saving…";
    try {
      const operation = {
        action: "configure_global",
        quiet_hours: quiet(),
        auto_close_merged_prs: $("#global-auto-close-prs").checked,
        boss_name: $("#global-boss-name").value.trim(),
        if_version: globalSettingsVersion,
      };
      const key = JSON.stringify([globalSettingsHost, operation]);
      if (globalSettingsPending?.key !== key)
        globalSettingsPending = { key, id: HeyBossUI.requestId() };
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
      globalQuietOriginal = value.quiet_hours;
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
