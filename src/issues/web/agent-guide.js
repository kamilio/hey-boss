"use strict";
// No resource data or extra requests. Fragments are unavailable to HTTP servers.
(() => {
  const guide = document.getElementById("hey-boss-agent-guide");
  if (!guide) return;
  const template = guide.textContent;
  const quote = value => "'" + value.replaceAll("'", "'\"'\"'") + "'";
  function update() {
    const url = new URL(location.href);
    // The paired root is Inbox, while the desktop root defaults to Issues.
    // Use a CLI command directly when that URL does not encode the visible view.
    const pairedInbox = document.getElementById("root") && url.pathname === "/" && !url.searchParams.has("task");
    const command = pairedInbox ? "hey-boss notif inbox --json" : "hey-boss lookup " + quote(url.href) + " --json";
    guide.textContent = template.replace("hey-boss lookup 'FULL_PAGE_URL' --json", command);
  }
  update();
  // These pages navigate without a reload; History API calls emit no browser event.
  for (const method of ["pushState", "replaceState"]) {
    const original = history[method];
    history[method] = function (...args) {
      const result = original.apply(this, args);
      update();
      return result;
    };
  }
  window.addEventListener("hashchange", update);
  window.addEventListener("popstate", update);
})();
