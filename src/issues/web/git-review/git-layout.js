
export function renderGitLayoutControl({ legacy = false } = {}) {
  return `<div class="git-layout ${legacy ? "segmented" : "layout"}" role="group" aria-label="Diff layout"><button type="button" class="active" data-layout="unified" aria-pressed="true">Unified</button><button type="button" data-layout="split" aria-pressed="false">Split</button></div>`;
}

export function mountGitLayout() {
  const buttons = [...document.querySelectorAll(".git-layout [data-layout]")];
  for (const button of buttons) button.addEventListener("click", () => {
    document.body.classList.toggle("split", button.dataset.layout === "split");
    for (const candidate of buttons) {
      const active = candidate === button;
      candidate.classList.toggle("active", active);
      candidate.setAttribute("aria-pressed", String(active));
    }
  });
}
