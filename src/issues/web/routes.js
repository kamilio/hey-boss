"use strict";
// This table is embedded from routes.json by both the Rust server and mobile build.
const HeyBossRoutes = (() => {
  const rules = /* ROUTE_DEFINITIONS */;
  const issueNumber = value => /^[1-9]\d*$/.test(value || "") && Number.isSafeInteger(Number(value));
  function resolve(url = location.href) {
    const address = new URL(url);
    const params = new URLSearchParams(address.hash.slice(1));
    const rule = rules.find(rule => rule.paths.includes(address.pathname) &&
      (!rule.query_selector || address.searchParams.get(rule.query_selector)) &&
      (!rule.present || params.get(rule.present)) &&
      Object.entries(rule.when || {}).every(([key, value]) => params.get(key) === value));
    if (!rule) return null;
    let id = (rule.query_selector ? address.searchParams.get(rule.query_selector) : params.get(rule.selector)) || "";
    if (rule.number && !issueNumber(id)) id = "";
    return {entity: id ? rule.entity : rule.collection, id,
      project: params.get("project") || "", host: params.get("host") || "", params};
  }
  return {resolve, issueNumber};
})();
if (typeof module !== "undefined") module.exports = HeyBossRoutes;
