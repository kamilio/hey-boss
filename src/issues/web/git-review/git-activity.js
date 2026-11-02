export function createGitActivity({ name, className, controls, insert, collapsed, reveal, focus, prefix = "", completedAction = "Show file" }) {
  const active = new Map(); let completion = "", notice, message, button;
  const render = () => {
    const items = [...active.values()], saving = items.filter(item => item.state === "pending" && item.action === "save").length, deleting = items.filter(item => item.state === "pending" && item.action === "delete").length, failed = items.filter(item => item.state === "failed").length;
    const parts = [];
    if (saving) parts.push(`Saving ${saving} comment${saving === 1 ? "" : "s"}…`);
    if (deleting) parts.push(`Deleting ${deleting} comment${deleting === 1 ? "" : "s"}…`);
    if (failed) parts.push(`${failed} action${failed === 1 ? "" : "s"} failed`);
    const status = parts.join(" · ") || completion, text = status ? (typeof prefix === "function" ? prefix() : prefix) + status : "";
    if (!notice && !text) return;
    if (!notice) {
      notice = document.createElement("div"); notice.className = `git-activity ${className}`;
      notice.innerHTML = '<span role="status" aria-live="polite" aria-atomic="true"></span><button type="button"></button>';
      insert(notice); message = notice.querySelector("span"); button = notice.querySelector("button"); button.setAttribute("aria-controls", controls);
    }
    if (!text && notice.contains(document.activeElement)) focus()?.focus({ preventScroll: true });
    notice.hidden = !text; notice.dataset.state = failed ? "error" : saving || deleting ? "pending" : "success";
    if (message.textContent !== text) message.textContent = text;
    const action = failed ? "Review" : saving || deleting ? "Show" : typeof completedAction === "function" ? completedAction() : completedAction;
    if (button.textContent !== action) button.textContent = action;
    button.setAttribute("aria-label", `${action} comment activity in ${name}`);
  };
  return {
    update(owner, state, action = "save") {
      if (state === "pending" || state === "failed") { active.set(owner, { owner, state, action }); completion = ""; }
      else { active.delete(owner); if (state === "success" && collapsed()) completion = action === "delete" ? "Comment deleted." : "Comment saved."; }
      render();
    },
    acknowledge() { completion = ""; render(); },
    refresh: render,
    show() {
      const items = [...active.values()], item = items.find(candidate => candidate.state === "failed") ?? items[0];
      reveal(); completion = ""; render();
      const target = item?.state === "failed" ? item.owner.querySelector(".submit-comment,.confirm-delete") : item?.owner.querySelector("textarea");
      (target?.isConnected && !target.disabled ? target : focus())?.focus();
    },
    get control() { return button; },
    get element() { return notice; },
  };
}
