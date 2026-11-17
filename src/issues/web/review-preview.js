(() => {
  const dialog = $("#review-preview"), frame = $("#preview-frame");
  const edited = new WeakSet();
  const close = () => {
    if ([...(frame.contentDocument?.querySelectorAll("textarea") || [])].some(input => input.offsetWidth && edited.has(input))
      && !confirm("Discard your unsaved edit?")) return;
    dialog.close();
  };
  $("#preview-close").onclick = close;
  dialog.oncancel = event => { event.preventDefault(); close(); };
  dialog.onclose = () => { frame.removeAttribute("src"); refresh(); };
  frame.onload = () => {
    const doc = frame.contentDocument;
    if (!doc || !["/code-review", "/commit-review"].includes(doc.location.pathname)) return;
    doc.documentElement.classList.add("embedded-review");
    doc.addEventListener("input", event => edited.add(event.target));
    doc.defaultView.addEventListener("keydown", event => {
      if (event.key === "Escape" && !event.defaultPrevented) { event.preventDefault(); close(); }
    });
    const task = doc.querySelector("#task-title");
    if (task) task.target = "_top";
  };
  window.addEventListener("hashchange", () => { if (dialog.open) dialog.close(); });
  document.addEventListener("click", event => {
    const link = event.target.closest("a[data-review-preview]");
    if (!link || event.defaultPrevented || event.button !== 0 || event.shiftKey || event.ctrlKey || event.metaKey || event.altKey) return;
    event.preventDefault();
    const title = new URL(link.href).pathname === "/code-review" ? "Code review" : "Commit review";
    $("#preview-title").textContent = frame.title = title;
    $("#preview-full").href = frame.src = link.href;
    dialog.showModal();
  });
})();
