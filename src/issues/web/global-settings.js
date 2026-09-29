"use strict";
let globalSettingsVersion = null,
  globalSettingsOriginal = null,
  globalAutoCloseOriginal = true,
  globalSelectedSkillsOriginal = [],
  globalSelectedSkillsDraft = new Set(),
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
  function skillsSelectionChanged() {
    const current = [...globalSelectedSkillsDraft].sort().join(",");
    const orig = [...globalSelectedSkillsOriginal].sort().join(",");
    return current !== orig;
  }
  function renderSkillsPanel(skillsReport) {
    const container = $("#global-skills-list");
    if (!container || !skillsReport) return;
    const globalSkills = skillsReport.global_skills || [];
    const projectSkills = skillsReport.project_skills || [];
    const maxLines = skillsReport.max_lines_policy || 120;
    const renderRow = (s, isProject = false) => {
      const checked = isProject ? true : globalSelectedSkillsDraft.has(s.name);
      const agents = ["codex", "claude", "agents"].map(a =>
        `<span class="skill-agent-badge ${s.agents?.[a] ? "is-present" : "is-missing"}">${a}</span>`
      ).join("");
      const warnings = (s.warnings || []).map(w =>
        `<span class="skill-warning-badge is-${esc(w.kind)}">${esc(w.message)}</span>`
      ).join("");
      return `<div class="skill-audit-row ${s.warnings?.length ? "has-warnings" : ""}">
        <label class="skill-select-label">
          ${isProject ? '<span class="skill-project-tag">Project</span>' : `<input type="checkbox" data-skill-select="${esc(s.name)}" ${checked ? "checked" : ""} ${s.name === "hey-boss" ? "disabled" : ""} />`}
          <strong>${esc(s.name)}</strong>
          <span class="skill-line-count ${s.line_count > maxLines ? "is-too-long" : ""}">${s.line_count} lines</span>
        </label>
        <div class="skill-badges">${agents}${warnings}</div>
      </div>`;
    };
    container.innerHTML = `
      ${globalSkills.map(s => renderRow(s, false)).join("")}
      ${projectSkills.length ? `<div class="skill-subhead">Project skills (policy max ${maxLines} lines)</div>${projectSkills.map(s => renderRow(s, true)).join("")}` : ""}
    `;
    container.querySelectorAll("[data-skill-select]").forEach(cb => {
      cb.onchange = () => {
        if (cb.checked) globalSelectedSkillsDraft.add(cb.dataset.skillSelect);
        else globalSelectedSkillsDraft.delete(cb.dataset.skillSelect);
        globalSettingsChanged();
      };
    });
  }
  function globalSettingsChanged() {
    const changed =
      globalSettingsOriginal !== null &&
      ($("#global-boss-name").value.trim() !== globalSettingsOriginal ||
       $("#global-auto-close-prs").checked !== globalAutoCloseOriginal ||
       skillsSelectionChanged());
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
      globalSelectedSkillsOriginal = value.skills?.selected || ["hey-boss", "stacked-prs"];
      globalSelectedSkillsDraft = new Set(globalSelectedSkillsOriginal);
      renderSkillsPanel(value.skills);
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
  if ($("#global-skills-sync-now")) {
    $("#global-skills-sync-now").onclick = async () => {
      if (globalSettingsSaving) return;
      const btn = $("#global-skills-sync-now");
      btn.disabled = true;
      $("#global-settings-state").textContent = "Syncing skills…";
      try {
        const value = await api(
          {
            action: "configure_global",
            boss_name: $("#global-boss-name").value.trim() || globalSettingsOriginal,
            auto_close_merged_prs: $("#global-auto-close-prs").checked,
            selected_skills: [...globalSelectedSkillsDraft],
            sync_skills: true,
            if_version: globalSettingsVersion,
          },
          model.project.id,
          HeyBossUI.requestId(),
          globalSettingsHost,
        );
        globalSettingsVersion = value.version;
        globalSelectedSkillsOriginal = value.skills?.selected || [...globalSelectedSkillsDraft];
        globalSelectedSkillsDraft = new Set(globalSelectedSkillsOriginal);
        renderSkillsPanel(value.skills);
        globalSettingsChanged();
        $("#global-settings-state").textContent = "Skills synced across Codex, Claude & Agents";
      } catch (err) {
        $("#global-settings-error").textContent = err.message;
        $("#global-settings-error").hidden = false;
      } finally {
        btn.disabled = false;
      }
    };
  }
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
