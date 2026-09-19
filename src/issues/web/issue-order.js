"use strict";
let issueDrag = null,
  issueDragFrame = null;
function issueOrderContext() {
  return {
    project: model.project.id,
    sequence: model.sequence,
    version: model.orderVersion,
    detail: model.route.issue,
  };
}
function clearIssueDrop() {
  $$(".issue-drop-before,.issue-drop-after").forEach((row) =>
    row.classList.remove("issue-drop-before", "issue-drop-after"),
  );
}
function cancelIssueDrag() {
  if (!issueDrag) return;
  const drag = issueDrag;
  issueDrag = null;
  model.orderDragging = false;
  cancelAnimationFrame(issueDragFrame);
  drag.row.classList.remove("issue-dragging");
  document.body.classList.remove("issue-sorting");
  clearIssueDrop();
  if (drag.handle.hasPointerCapture(drag.pointer))
    drag.handle.releasePointerCapture(drag.pointer);
}
async function moveIssue(
  number,
  anchor,
  placement,
  context = issueOrderContext(),
) {
  if (
    model.orderSaving ||
    context.project !== model.project.id ||
    context.sequence !== model.sequence
  )
    return;
  model.orderSaving = true;
  $$(".issue-order-handle").forEach((button) => (button.disabled = true));
  try {
    await mutate(
      {
        action: "move",
        number,
        [placement]: anchor,
        if_order_version: context.version,
      },
      context.project,
    );
    if (context.sequence === model.sequence) toast("Issue order updated");
  } catch (error) {
    if (context.sequence === model.sequence) toast(error.message, true);
  } finally {
    try {
      if (context.sequence === model.sequence) {
        if (context.detail) saveComment();
        const result = await api(context.detail ? {action:"view",number:context.detail} : listOperation(), context.project);
        if (context.sequence === model.sequence) {
          if (context.detail) renderDetail(result);else renderList(result);
          const handle = $(`.issue-order-handle[data-move-issue="${number}"]`, $(context.detail ? "#subtask-list" : "#issue-list"));
          handle?.focus({ preventScroll: true });
          handle?.scrollIntoView({ block: "nearest", behavior: "instant" });
        }
      }
    } catch (error) {
      if (context.sequence === model.sequence) toast(error.message, true);
    }
    model.orderSaving = false;
    $$(".issue-order-handle").forEach((button) => (button.disabled = false));
  }
}
function updateIssueDrop() {
  const drag = issueDrag;
  if (!drag?.active) return;
  clearIssueDrop();
  const list = drag.list,
    bounds = list.getBoundingClientRect();
  const target = document
    .elementsFromPoint(drag.x, drag.y)
    .map((el) => el.closest(".issue-row[data-issue-number]"))
    .find((row) => row && list.contains(row));
  drag.target = null;
  if (target && target !== drag.row) {
    const box = target.getBoundingClientRect();
    drag.placement = drag.y < box.top + box.height / 2 ? "before" : "after";
    drag.target = Number(target.dataset.issueNumber);
    target.classList.add(`issue-drop-${drag.placement}`);
  } else if (drag.x >= bounds.left && drag.x <= bounds.right) {
    const rows = $$(".issue-row", list),
      first = rows[0],
      last = rows.at(-1);
    const edge =
      drag.y < bounds.top ? first : drag.y > bounds.bottom ? last : null;
    if (edge && edge !== drag.row) {
      drag.placement = edge === first ? "before" : "after";
      drag.target = Number(edge.dataset.issueNumber);
      edge.classList.add(`issue-drop-${drag.placement}`);
    }
  }
}
function scrollIssueDrag() {
  const drag = issueDrag;
  if (!drag?.active) return;
  if (drag.y < 90) window.scrollBy(0, -12);
  else if (drag.y > innerHeight - 90) window.scrollBy(0, 12);
  updateIssueDrop();
  issueDragFrame = requestAnimationFrame(scrollIssueDrag);
}
document.addEventListener("pointerdown", (event) => {
  const handle = event.target.closest(".issue-order-handle");
  if (!handle || !handle.closest("#issue-list,#subtask-list") || event.button !== 0 || !event.isPrimary || model.orderSaving)
    return;
  event.preventDefault();
  handle.focus({ preventScroll: true });
  issueDrag = {
    handle,
    list: handle.closest("#issue-list,#subtask-list"),
    row: handle.closest(".issue-row"),
    number: Number(handle.dataset.moveIssue),
    pointer: event.pointerId,
    startX: event.clientX,
    startY: event.clientY,
    x: event.clientX,
    y: event.clientY,
    active: false,
    target: null,
    context: issueOrderContext(),
  };
  model.orderDragging = true;
  handle.setPointerCapture(event.pointerId);
});
document.addEventListener("pointermove", (event) => {
  const drag = issueDrag;
  if (!drag || event.pointerId !== drag.pointer) return;
  drag.x = event.clientX;
  drag.y = event.clientY;
  if (
    !drag.active &&
    Math.hypot(drag.x - drag.startX, drag.y - drag.startY) >= 5
  ) {
    drag.active = true;
    drag.row.classList.add("issue-dragging");
    document.body.classList.add("issue-sorting");
    scrollIssueDrag();
  }
  if (drag.active) {
    event.preventDefault();
    updateIssueDrop();
  }
});
document.addEventListener("pointerup", (event) => {
  const drag = issueDrag;
  if (!drag || event.pointerId !== drag.pointer) return;
  if (drag.active) event.preventDefault();
  cancelIssueDrag();
  if (drag.active && drag.target)
    moveIssue(drag.number, drag.target, drag.placement, drag.context);
});
for (const event of ["pointercancel", "lostpointercapture"])
  document.addEventListener(event, cancelIssueDrag);
document.addEventListener("keydown", (event) => {
  const handle = event.target.closest(".issue-order-handle");
  if (
    !handle ||
    !handle.closest("#issue-list,#subtask-list") ||
    !["ArrowUp", "ArrowDown"].includes(event.key) ||
    event.ctrlKey ||
    event.metaKey
  )
    return;
  event.preventDefault();
  if (model.orderSaving || issueDrag) return;
  const number = Number(handle.dataset.moveIssue),
    issues = handle.closest("#subtask-list") ? model.detail.subtasks.filter(i=>!i.deleted_at) : model.issues,
    index = issues.findIndex((issue) => issue.number === number),
    direction = event.key === "ArrowUp" ? -1 : 1,
    target = issues[index + direction];
  if (target)
    moveIssue(number, target.number, direction < 0 ? "before" : "after");
});
window.addEventListener("hashchange", cancelIssueDrag);
window.addEventListener("blur", cancelIssueDrag);
document.addEventListener("visibilitychange", () => {
  if (document.hidden) cancelIssueDrag();
});

document.addEventListener(
  "keydown",
  (event) => {
    if (event.key === "Escape" && issueDrag) {
      event.preventDefault();
      event.stopPropagation();
      cancelIssueDrag();
    }
  },
  true,
);
