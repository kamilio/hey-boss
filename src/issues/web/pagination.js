"use strict";
// Shared by the desktop lists and the phone Inbox. Filter before taking a page.
globalThis.HeyBossPagination = (() => {
  const size = 1000;
  function number(value) {
    const result = Number(value);
    return Number.isSafeInteger(result) && result > 0 ? result : 1;
  }
  function page(items, requested) {
    const total = items.length, pages = Math.max(1, Math.ceil(total / size));
    const current = Math.min(number(requested), pages), offset = (current - 1) * size;
    return {items: items.slice(offset, offset + size), total, pages, current,
      start: total ? offset + 1 : 0, end: Math.min(offset + size, total)};
  }
  return {number, page, size};
})();
