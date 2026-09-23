"use strict";
class TagInput {
  constructor(root, input, candidates, onchange = () => {}) {
    this.root = root;
    this.input = input;
    this.candidates = candidates;
    this.onchange = onchange;
    this.selected = [];
    this.chips = root.querySelector(".tag-chips");
    this.options = root.querySelector(".tag-suggestions");
    input.addEventListener("input", () => {
      this.renderOptions();
      onchange();
    });
    input.addEventListener("focus", () => this.renderOptions());
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && input.value.trim()) {
        event.preventDefault();
        this.commit();
      } else if (event.key === "Escape" && !this.options.hidden) {
        event.preventDefault();
        event.stopPropagation();
        this.options.hidden = true;
      } else if (
        event.key === "Backspace" &&
        !input.value &&
        this.selected.length
      ) {
        this.selected.pop();
        this.render();
        onchange();
      } else if (event.key === "ArrowDown" && !this.options.hidden) {
        event.preventDefault();
        this.options.querySelector("button")?.focus();
      }
    });
    root.addEventListener("focusout", () =>
      setTimeout(() => {
        if (!root.contains(document.activeElement)) this.options.hidden = true;
      }, 0),
    );
    this.chips.addEventListener("click", (event) => {
      const button = event.target.closest("[data-remove-tag]");
      if (!button) return;
      this.selected = this.selected.filter(
        (tag) => tag !== button.dataset.removeTag,
      );
      this.render();
      input.focus();
      onchange();
    });
    this.options.addEventListener("click", (event) => {
      const button = event.target.closest("[data-add-tag]");
      if (!button) return;
      this.add(button.dataset.addTag);
      input.focus();
    });
    this.options.addEventListener("keydown", (event) => {
      const buttons = [...this.options.querySelectorAll("button")];
      const index = buttons.indexOf(document.activeElement);
      if (event.key === "Escape" || (event.key === "ArrowUp" && index === 0)) {
        event.preventDefault();
        event.stopPropagation();
        input.focus();
        if (event.key === "Escape") this.options.hidden = true;
      } else if (["ArrowDown", "ArrowUp"].includes(event.key)) {
        event.preventDefault();
        buttons[index + (event.key === "ArrowDown" ? 1 : -1)]?.focus();
      }
    });
  }
  set(values) {
    this.selected = [...new Set(values)];
    this.input.value = "";
    this.render();
  }
  values() {
    return [
      ...new Set([
        ...this.selected,
        ...this.input.value
          .split(",")
          .map((v) => v.trim())
          .filter(Boolean),
      ]),
    ];
  }
  commit() {
    this.selected = this.values();
    this.input.value = "";
    this.render();
    this.onchange();
  }
  add(tag) {
    if (!this.selected.includes(tag)) this.selected.push(tag);
    this.input.value = "";
    this.render();
    this.onchange();
  }
  render() {
    this.chips.innerHTML = this.selected
      .map(
        (tag) =>
          `<span class="tag-chip">${label(tag)}<button type="button" data-remove-tag="${esc(tag)}" aria-label="Remove ${esc(tag)} tag">${icon("x")}</button></span>`,
      )
      .join("");
    this.renderOptions();
  }
  renderOptions() {
    const query = this.input.value.trim();
    const available = [...new Set(this.candidates())].filter(
      (tag) => !specialIssueTags.has(tag) && !this.selected.includes(tag),
    );
    const matches = available
      .filter((tag) =>
        tag.toLocaleLowerCase().includes(query.toLocaleLowerCase()),
      )
      .sort()
      .slice(0, 30);
    const create =
      query &&
      !query.includes(",") &&
      !specialIssueTags.has(query) &&
      !this.selected.includes(query) &&
      !available.includes(query);
    this.options.innerHTML =
      matches
        .map(
          (tag) =>
            `<button type="button" data-add-tag="${esc(tag)}">${label(tag)}${icon("plus")}</button>`,
        )
        .join("") +
      (create
        ? `<button type="button" data-add-tag="${esc(query)}">${icon("plus")}Create ${esc(query)}</button>`
        : "");
    this.options.hidden =
      !this.root.contains(document.activeElement) ||
      !this.options.children.length;
  }
}

let issueTagPicker = null;
function renderTagSidebar(issue) {
  const deleted = !!issue.deleted_at;
  return `<div class="side-section tag-section"><h2 class="side-heading">Tags${deleted ? "" : `<button class="icon-button" type="button" data-tag-picker aria-label="Assign tags" aria-haspopup="dialog">${icon("plus")}</button>`}</h2><div class="side-labels">${renderIssueTagChips(issue)}</div>${deleted ? "" : '<button class="button small link-button" type="button" data-tag-picker>Add tags</button>'}</div>`;
}
function renderIssueTagChips(issue) {
  return issue.labels.length
    ? issue.labels
        .map(
          (tag) =>
            `<span class="tag-chip">${label(tag)}${issue.deleted_at || (specialIssueTags.has(tag) && model.actor.id !== "human:boss") ? "" : `<button type="button" data-remove-issue-tag="${esc(tag)}" aria-label="Remove ${esc(tag)} tag">${icon("x")}</button>`}</span>`,
        )
        .join("")
    : '<span class="muted-text">No tags yet</span>';
}
function closeIssueTagPicker() {
  $("#issue-tag-picker")?.remove();
  issueTagPicker = null;
}
function openIssueTagPicker(trigger) {
  if (issueTagPicker) {
    closeIssueTagPicker();
    return;
  }
  issueTagPicker = {
    project: model.project.id,
    host: model.route.host,
    issue: model.detail.issue,
    busy: false,
    trigger,
  };
  $(".tag-section").insertAdjacentHTML(
    "beforeend",
    `<div class="tag-popover" id="issue-tag-picker" role="dialog" aria-label="Assign tags"><div class="popover-heading"><strong>Assign tags</strong><button class="icon-button" type="button" data-close-tags aria-label="Close tag picker">${icon("x")}</button></div><input id="issue-tag-search" type="search" class="text-input" aria-label="Find or create a tag" placeholder="Search or create a tag…" autocomplete="off"><div id="issue-tag-options" role="group" aria-label="Available tags"></div><p id="issue-tag-error" class="form-error" role="alert" hidden></p><p class="field-help">Changes save immediately.</p></div>`,
  );
  $("#issue-tag-search").oninput = renderIssueTagOptions;
  $("#issue-tag-search").onkeydown = (event) => {
    if (event.key === "Enter" && event.target.value.trim()) {
      event.preventDefault();
      changeIssueTag(event.target.value.trim(), true);
    }
  };
  renderIssueTagOptions();
  $("#issue-tag-search").focus();
  $("#issue-tag-picker").scrollIntoView({block: "nearest"});
}
function renderIssueTagOptions() {
  const ctx = issueTagPicker;
  if (!ctx) return;
  const query = $("#issue-tag-search").value.trim();
  const tags = [...new Set([...specialIssueTags.keys(), ...model.labels, ...ctx.issue.labels])].sort();
  $("#issue-tag-options").innerHTML =
    tags
      .filter((tag) =>
        tag.toLocaleLowerCase().includes(query.toLocaleLowerCase()),
      )
      .map(
        (tag) =>
          `<label class="tag-option"><input type="checkbox" data-issue-tag="${esc(tag)}" ${ctx.issue.labels.includes(tag) ? "checked" : ""} ${ctx.busy || (specialIssueTags.has(tag) && model.actor.id !== "human:boss") ? "disabled" : ""}>${label(tag)}</label>`,
      )
      .join("") +
    (query && !tags.includes(query)
      ? `<button class="button tag-create" type="button" data-new-issue-tag="${esc(query)}" ${ctx.busy ? "disabled" : ""}>${icon("plus")}Create ${esc(query)}</button>`
      : "");
  $$("[data-issue-tag]").forEach(
    (input) =>
      (input.onchange = () =>
        changeIssueTag(input.dataset.issueTag, input.checked)),
  );
}
let issueTagChanges = Promise.resolve();
function changeIssueTag(tag, add) {
  const project = model.project.id,
    number = model.detail.issue.number,
    host = model.route.host;
  issueTagChanges = issueTagChanges.then(() => {
    if (model.project.id !== project || model.detail?.issue.number !== number || model.route.host !== host)
      return;
    return applyIssueTag(tag, add);
  });
  return issueTagChanges;
}
async function applyIssueTag(tag, add) {
  const ctx = issueTagPicker || {
    project: model.project.id,
    host: model.route.host,
    issue: model.detail.issue,
    busy: false,
  };
  if (ctx.busy) return;
  ctx.busy = true;
  if (issueTagPicker === ctx) {
    $("#issue-tag-error").hidden = true;
    renderIssueTagOptions();
  }
  try {
    const special = specialIssueTags.get(tag);
    if (special) {
      if (model.actor.id !== "human:boss") return;
      if (model.project.id !== ctx.project || model.detail?.issue.number !== ctx.issue.number || model.route.host !== ctx.host) return;
    }
    const value = await mutate(
      special ? {action: special.action, number: ctx.issue.number, enabled: add, if_version: ctx.issue.version} : {
        action: "edit",
        number: ctx.issue.number,
        title: null,
        body: null,
        add_labels: add ? [tag] : [],
        remove_labels: add ? [] : [tag],
        if_version: ctx.issue.version,
      },
      ctx.project, ctx.host,
    );
    ctx.issue = value.issue;
    if (
      model.project.id === ctx.project &&
      model.detail?.issue.number === ctx.issue.number &&
      model.route.host === ctx.host
    ) {
      model.detail.issue = value.issue;
      $(".side-labels").innerHTML = renderIssueTagChips(value.issue);
      $("[data-issue-version]").textContent = `Revision ${value.issue.version}`;
      if (add && !model.labels.includes(tag)) model.labels.push(tag);
      await refreshProjects(ctx.project);
    }
    if (issueTagPicker === ctx) {
      $("#issue-tag-search").value = "";
      $("#issue-tag-search").focus();
    }
  } catch (error) {
    if (issueTagPicker === ctx) {
      $("#issue-tag-error").textContent = error.message;
      $("#issue-tag-error").hidden = false;
    } else toast(error.message, true);
    if (error.code === "conflict") {
      const latest = await api(
        { action: "view", number: ctx.issue.number },
        ctx.project, null, ctx.host,
      ).catch(() => null);
      if (latest) {
        ctx.issue = latest.issue;
        if (
          model.project.id === ctx.project &&
          model.detail?.issue.number === ctx.issue.number &&
          model.route.host === ctx.host
        ) {
          model.detail.issue = latest.issue;
          $(".side-labels").innerHTML = renderIssueTagChips(latest.issue);
          $("[data-issue-version]").textContent =
            `Revision ${latest.issue.version}`;
        }
      }
    }
  } finally {
    ctx.busy = false;
    if (issueTagPicker === ctx) {
      renderIssueTagOptions();
      if (specialIssueTags.has(tag)) $(`[data-issue-tag="${CSS.escape(tag)}"]`)?.focus();
    }
  }
}
document.addEventListener("click", (event) => {
  const trigger = event.target.closest("[data-tag-picker]");
  if (trigger) {
    openIssueTagPicker(trigger);
    return;
  }
  const remove = event.target.closest("[data-remove-issue-tag]");
  if (remove) {
    changeIssueTag(remove.dataset.removeIssueTag, false);
    return;
  }
  const create = event.target.closest("[data-new-issue-tag]");
  if (create) {
    changeIssueTag(create.dataset.newIssueTag, true);
    return;
  }
  if (event.target.closest("[data-close-tags]")) {
    const trigger = issueTagPicker?.trigger;
    closeIssueTagPicker();
    trigger?.focus();
    return;
  }
  if (issueTagPicker && !issueTagPicker.busy && !event.target.closest("#issue-tag-picker"))
    closeIssueTagPicker();
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && issueTagPicker && !issueTagPicker.busy) {
    event.preventDefault();
    const trigger = issueTagPicker.trigger;
    closeIssueTagPicker();
    trigger?.focus();
  }
});
