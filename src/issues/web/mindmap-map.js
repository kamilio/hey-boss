(function (root) {
  "use strict";
  const WIDTH = 240,
    HEIGHT = 84,
    STEP = 320,
    GAP = 28;
  const colors = [
    "#6682e8",
    "#36a791",
    "#bf79ca",
    "#d79943",
    "#dc7984",
    "#529cc5",
  ];
  const esc = (s) =>
    String(s ?? "").replace(
      /[&<>"']/g,
      (c) =>
        ({
          "&": "&amp;",
          "<": "&lt;",
          ">": "&gt;",
          '"': "&quot;",
          "'": "&#39;",
        })[c],
    );
  const kinds = new Map([
    ["text", ["Topic", "subtasks"]],
    ["markdown", ["Note", "docs"]],
    ["issue", ["Issue", "issue"]],
    ["pr", ["Pull request", "pull-request"]],
    ["notification", ["Notice", "bell"]],
    ["project", ["Project", "folder"]],
  ]);
  function kindIcon(kind) {
    if (!kinds.has(kind)) kind = "text";
    const [label, icon] = kinds.get(kind);
    return `<span class="mindmap-kind mindmap-kind-${kind}" role="img" aria-label="${label}" title="${label}">${HeyBossUI.icon(icon)}</span>`;
  }
  function displayTitle(node) {
    if (
      node.kind !== "pr" ||
      node.title.replace(/\/+$/, "") !== node.reference.replace(/\/+$/, "")
    ) return node.title;
    try {
      const url = new URL(node.reference), path = url.pathname.replace(/\/+$/, "");
      const number = path.match(/\/(?:pull|merge_requests)\/(\d+)$/)?.[1];
      return number ? `#${number}` : `${url.hostname}${path ? ` · ${path.split("/").pop()}` : ""}`;
    } catch {
      return "Pull request";
    }
  }
  function layout(nodes, collapsed = new Set(), matches = null) {
    const index = new Map(nodes.map((n) => [n.id, n])),
      children = new Map();
    for (const n of nodes) {
      const parent = index.has(n.parent_id) ? n.parent_id : "";
      if (!children.has(parent)) children.set(parent, []);
      children.get(parent).push(n);
    }
    const items = [],
      positions = new Map();
    let cursor = 0,
      width = WIDTH;
    function visit(node, depth, color) {
      if (matches && !matches.has(node.id)) return null;
      const kids = children.get(node.id) || [],
        expanded = matches !== null || !collapsed.has(node.id);
      const item = {
        ...node,
        x: STEP * depth,
        y: 0,
        color,
        childCount: kids.length,
        expanded,
      };
      items.push(item);
      positions.set(item.id, item);
      width = Math.max(width, item.x + WIDTH);
      const shown = expanded
        ? kids.map((n) => visit(n, depth + 1, color)).filter(Boolean)
        : [];
      if (!shown.length) {
        item.y = cursor;
        cursor += HEIGHT + GAP;
      } else item.y = (shown[0].y + shown[shown.length - 1].y) / 2;
      return item;
    }
    (children.get("") || []).forEach((n, i) =>
      visit(n, 1, colors[i % colors.length]),
    );
    const height = Math.max(HEIGHT, cursor - GAP);
    return {
      items,
      positions,
      width,
      height,
      rootY: (height - HEIGHT) / 2,
    };
  }
  function inViewport(item, view, margin = 100) {
    return (
      item.x + WIDTH >= view.x - margin &&
      item.x <= view.x + view.width + margin &&
      item.y + HEIGHT >= view.y - margin &&
      item.y <= view.y + view.height + margin
    );
  }
  function indexGraph(items, links, assigneeName) {
    const nodes = new Map(items.map((node) => [node.id, node])),
      titles = new Map(items.map((node) => [node.id, displayTitle(node)])),
      incidents = new Map(),
      documents = new Map();
    for (const link of links)
      for (const id of [link.from, link.to]) {
        if (!incidents.has(id)) incidents.set(id, []);
        incidents.get(id).push(link);
      }
    for (const node of nodes.values())
      documents.set(
        node.id,
        [
          node.title,
          node.original_title,
          titles.get(node.id),
          node.body,
          node.state,
          node.assignee,
          node.kind === "issue" && node.available !== false
            ? assigneeName(node.assignee)
            : "",
          node.alias,
          node.kind,
          node.reference,
          node.reference_project,
          node.reference_project_name,
          ...(incidents.get(node.id) || []).flatMap((link) => [
            link.kind,
            link.description,
            nodes.get(link.from)?.title,
            nodes.get(link.to)?.title,
            titles.get(link.from),
            titles.get(link.to),
          ]),
        ]
          .join(" ")
          .toLowerCase(),
      );
    return { nodes, incidents, documents };
  }
  class Mindmap {
    constructor(element, { select, toggle, escape }) {
      this.element = element;
      this.select = select;
      this.toggle = toggle;
      this.escape = escape;
      this.camera = { x: 0, y: 0, scale: 1 };
      this.cards = new Map();
      this.points = new Map();
      this.frame = null;
      element.innerHTML =
        '<div class="map-world"><svg class="map-edges" aria-hidden="true"></svg><div class="map-cards"></div></div><p class="map-empty" hidden></p>';
      this.world = element.querySelector(".map-world");
      this.edges = element.querySelector(".map-edges");
      this.layer = element.querySelector(".map-cards");
      this.overview = document.getElementById("map-overview");
      this.overview?.addEventListener("click", (e) => {
        if (!this.data) return;
        const box = this.overview.querySelector("canvas").getBoundingClientRect(),
          d = this.data,
          c = this.camera;
        const x = e.detail
          ? Math.max(0, Math.min(1, (e.clientX - box.left) / box.width)) * d.width
          : d.width / 2;
        const y = e.detail
          ? Math.max(0, Math.min(1, (e.clientY - box.top) / box.height)) * d.height
          : d.height / 2;
        c.x = element.clientWidth / 2 - x * c.scale;
        c.y = element.clientHeight / 2 - y * c.scale;
        this.schedule();
      });
      element.addEventListener("click", (e) => {
        if (
          e.detail !== 0 &&
          performance.now() < (this.suppressClickUntil || 0)
        ) {
          e.preventDefault();
          return;
        }
        const branch = e.target.closest("[data-map-toggle]");
        if (branch) {
          this.toggle(branch.dataset.mapToggle);
          return;
        }
        const card = e.target.closest("[data-map-node]");
        if (card) this.select(card.dataset.mapNode);
      });
      element.addEventListener("focusin", (e) => {
        const id = e.target.dataset.mapNode || e.target.dataset.mapToggle || e.target.dataset.mapResource,
          item = this.data?.positions.get(id);
        if (!item) return;
        const c = this.camera,
          inspector = document.getElementById("map-inspector"),
          reserved =
            element.clientWidth > 700 && inspector && !inspector.hidden
              ? inspector.offsetWidth + 28
              : 0;
        const x = item.x * c.scale + c.x,
          y = item.y * c.scale + c.y;
        if (
          x < 0 ||
          y < 0 ||
          x + WIDTH * c.scale > element.clientWidth - reserved ||
          y + HEIGHT * c.scale > element.clientHeight
        )
          this.focus(id, false);
      });
      element.addEventListener(
        "wheel",
        (e) => {
          e.preventDefault();
          if (e.ctrlKey || e.metaKey)
            this.zoom(Math.exp(-e.deltaY * 0.005), e.clientX, e.clientY);
          else {
            this.camera.x -= e.deltaX;
            this.camera.y -= e.deltaY;
            this.schedule();
          }
        },
        { passive: false },
      );
      element.addEventListener("pointerdown", (e) => {
        if (!this.points.size) this.suppressClickUntil = 0;
        const control = e.target.closest("button,a");
        if (e.button !== 0 || (control && e.pointerType !== "touch")) return;
        this.points.set(e.pointerId, {
          x: e.clientX,
          y: e.clientY,
          startX: e.clientX,
          startY: e.clientY,
        });
        if (!control) {
          element.setPointerCapture(e.pointerId);
          element.classList.add("panning");
        }
      });
      element.addEventListener("pointermove", (e) => {
        const last = this.points.get(e.pointerId);
        if (!last) return;
        const pair = [...this.points.values()],
          distance =
            pair.length === 2
              ? Math.hypot(pair[0].x - pair[1].x, pair[0].y - pair[1].y)
              : 0;
        this.points.set(e.pointerId, { ...last, x: e.clientX, y: e.clientY });
        if (
          !distance &&
          Math.hypot(e.clientX - last.startX, e.clientY - last.startY) < 5
        )
          return;
        this.suppressClickUntil = performance.now() + 500;
        element.setPointerCapture(e.pointerId);
        element.classList.add("panning");
        if (distance) {
          const next = [...this.points.values()];
          const oldX = (pair[0].x + pair[1].x) / 2,
            oldY = (pair[0].y + pair[1].y) / 2;
          const newX = (next[0].x + next[1].x) / 2,
            newY = (next[0].y + next[1].y) / 2;
          this.zoom(
            Math.hypot(next[0].x - next[1].x, next[0].y - next[1].y) / distance,
            oldX,
            oldY,
          );
          this.camera.x += newX - oldX;
          this.camera.y += newY - oldY;
        } else {
          this.camera.x += e.clientX - last.x;
          this.camera.y += e.clientY - last.y;
          this.schedule();
        }
      });
      for (const event of ["pointerup", "pointercancel", "lostpointercapture"])
        element.addEventListener(event, (e) => {
          // Touch capture can move from a card to the map while a drag continues.
          if (event === "lostpointercapture" && e.target !== element) return;
          this.points.delete(e.pointerId);
          if (!this.points.size) element.classList.remove("panning");
        });
      element.addEventListener("keydown", (e) => {
        if (e.target.closest("button,a,input")) return;
        const moves = {
          ArrowLeft: [70, 0],
          ArrowRight: [-70, 0],
          ArrowUp: [0, 70],
          ArrowDown: [0, -70],
        };
        if (moves[e.key]) {
          e.preventDefault();
          this.camera.x += moves[e.key][0];
          this.camera.y += moves[e.key][1];
          this.schedule();
        } else if (e.key === "+" || e.key === "=") {
          e.preventDefault();
          this.zoom(1.2);
        } else if (e.key === "-") {
          e.preventDefault();
          this.zoom(1 / 1.2);
        } else if (e.key === "0" || e.key === "Home") {
          e.preventDefault();
          this.fit();
        } else if (e.key === "Escape") this.escape();
      });
      this.resize = new ResizeObserver(() => this.schedule());
      this.resize.observe(element);
    }
    update(
      nodes,
      {
        collapsed,
        matches,
        hits,
        currentHit,
        project,
        selected,
        links,
        assigneeName,
      },
    ) {
      const changed = this.project?.id !== project.id;
      this.data = layout(nodes, collapsed, matches);
      this.project = project;
      this.selected = selected;
      this.links = links;
      this.hits = hits;
      this.currentHit = currentHit;
      this.assigneeName = assigneeName;
      if (this.overview) {
        this.overview.hidden = this.data.items.length === 0;
        this.overviewCache = document.createElement("canvas");
        this.overviewCache.width = 160;
        this.overviewCache.height = 96;
        const ctx = this.overviewCache.getContext("2d"),
          d = this.data,
          sx = 160 / d.width,
          sy = 96 / d.height;
        for (const item of d.items) {
          ctx.fillStyle = item.color;
          ctx.fillRect(
            item.x * sx,
            item.y * sy,
            Math.max(2, WIDTH * sx),
            Math.max(1, HEIGHT * sy),
          );
        }
      }
      const empty = this.element.querySelector(".map-empty");
      empty.hidden = this.data.items.length !== 0;
      empty.textContent = matches
        ? "No matching topics or relationships."
        : "Add your first topic with hey-boss mm add.";
      if (changed) {
        this.fit();
        if (this.camera.scale < 0.8 && this.data.items.length) {
          const item = this.data.items[0],
            box = this.element.getBoundingClientRect();
          this.camera = {
            scale: 0.8,
            x: box.width / 2 - (item.x + WIDTH / 2) * 0.8,
            y: box.height / 2 - (item.y + HEIGHT / 2) * 0.8,
          };
          this.schedule();
        }
      } else this.schedule();
    }
    fit() {
      if (!this.data) return;
      const box = this.element.getBoundingClientRect(),
        d = this.data,
        inspector = document.getElementById("map-inspector"),
        reserved =
          box.width > 700 && this.selected && inspector && !inspector.hidden
            ? inspector.offsetWidth + 28
            : 0,
        width = box.width - reserved;
      const scale = Math.max(
        0.18,
        Math.min(1, (width - 100) / d.width, (box.height - 100) / d.height),
      );
      this.camera = {
        scale,
        x: (width - d.width * scale) / 2,
        y: (box.height - d.height * scale) / 2,
      };
      this.schedule();
    }
    focus(id, takeFocus = true) {
      const item = this.data?.positions.get(id);
      if (!item) return false;
      const box = this.element.getBoundingClientRect(),
        scale = Math.max(0.8, this.camera.scale),
        inspector = document.getElementById("map-inspector");
      const reserved =
        box.width > 700 && inspector && !inspector.hidden
          ? inspector.offsetWidth + 28
          : 0;
      this.camera = {
        scale,
        x: (box.width - reserved) / 2 - (item.x + WIDTH / 2) * scale,
        y: box.height / 2 - (item.y + HEIGHT / 2) * scale,
      };
      this.draw();
      if (takeFocus)
        this.cards
          .get(id)
          ?.querySelector("[data-map-node]")
          ?.focus({ preventScroll: true });
      return true;
    }
    zoom(factor, clientX, clientY) {
      const box = this.element.getBoundingClientRect(),
        x = clientX == null ? box.width / 2 : clientX - box.left,
        y = clientY == null ? box.height / 2 : clientY - box.top;
      const c = this.camera,
        next = Math.max(0.18, Math.min(2, c.scale * factor)),
        ratio = next / c.scale;
      c.x = x - (x - c.x) * ratio;
      c.y = y - (y - c.y) * ratio;
      c.scale = next;
      this.schedule();
    }
    schedule() {
      if (this.frame === null)
        this.frame = requestAnimationFrame(() => {
          this.frame = null;
          this.draw();
        });
    }
    draw() {
      if (!this.data || this.element.hidden || !this.element.clientWidth)
        return;
      const c = this.camera,
        d = this.data,
        view = {
          x: -c.x / c.scale,
          y: -c.y / c.scale,
          width: this.element.clientWidth / c.scale,
          height: this.element.clientHeight / c.scale,
        };
      this.world.style.transform = `translate(${c.x}px,${c.y}px) scale(${c.scale})`;
      this.edges.setAttribute("width", d.width);
      this.edges.setAttribute("height", d.height);
      const keep = new Set(),
        paths = [];
      const curve = (a, b, color, dashed = false, label = "") => {
        // Avoid thousands of overlapping offscreen branches at a large parent.
        if (!inViewport(b, view, 200) || (dashed && !inViewport(a, view, 200)))
          return;
        if (
          Math.max(a.x, b.x) + WIDTH < view.x - 300 ||
          Math.min(a.x, b.x) > view.x + view.width + 300 ||
          Math.max(a.y, b.y) + HEIGHT < view.y - 300 ||
          Math.min(a.y, b.y) > view.y + view.height + 300
        )
          return;
        const sameColumn = dashed && a.x === b.x,
          forward = b.x >= a.x,
          x1 = a.x + (forward ? WIDTH : 0),
          y1 = a.y + HEIGHT / 2,
          x2 = b.x + (sameColumn || !forward ? WIDTH : 0),
          y2 = b.y + HEIGHT / 2,
          m = sameColumn ? x1 + 80 : (x1 + x2) / 2;
        paths.push(
          `<path d="M${x1} ${y1} C${m} ${y1},${m} ${y2},${x2} ${y2}" stroke="${color}" ${dashed ? 'class="map-link" marker-end="url(#map-arrow)"' : ""}>${label ? `<title>${esc(label)}</title>` : ""}</path>`,
        );
      };
      const root = {
        id: "__project__",
        x: 0,
        y: d.rootY,
        title: this.project.name,
        project: true,
        color: colors[0],
      };
      for (const item of d.items.length ? [root, ...d.items] : []) {
        const parent = d.positions.get(item.parent_id) || root;
        if (!item.project) curve(parent, item, item.color);
        const existing = this.cards.get(item.id);
        if (
          !inViewport(item, view) &&
          !existing?.contains(document.activeElement)
        )
          continue;
        keep.add(item.id);
        const source =
          item.kind === "issue" && item.reference_project !== this.project.id
            ? item.reference_project_name || item.reference_project
            : "";
        const meta = item.project
          ? ""
          : item.kind === "issue"
            ? `${source ? `${source} · ` : ""}#${item.reference} · ${item.state || "Unavailable"}${item.assignee ? ` · ${this.assigneeName(item.assignee)}` : ""}`
            : item.childCount
              ? `${item.childCount} topics`
              : item.alias || "";
        const title = displayTitle(item);
        const heading = `${kindIcon(item.project ? "project" : item.kind)}<span class="${item.project ? "map-project-title" : "map-card-title"}">${esc(title)}</span>`;
        const html = item.project
          ? `<div class="map-card-heading">${heading}</div>`
          : `<button class="map-card${item.kind === "pr" ? " map-pr-card" : ""}" data-map-node="${esc(item.id)}" title="${esc(title)} · ${kinds.get(item.kind)?.[0] || "Topic"}${meta ? ` · ${esc(meta)}` : ""}" aria-label="${esc(title)}${item.assignee ? `, assigned to ${esc(this.assigneeName(item.assignee))}` : ""}" ${item.id === this.selected ? 'aria-pressed="true"' : ""}><span class="map-card-heading">${heading}</span>${meta ? `<span class="map-card-meta">${esc(meta)}</span>` : ""}</button>${item.kind === "pr" ? `<a class="map-pr-open" data-map-resource="${esc(item.id)}" href="${esc(item.reference)}" target="_blank" rel="noopener noreferrer" title="Open pull request" aria-label="Open pull request ${esc(title)}">↗</a>` : ""}${item.childCount ? `<button class="map-branch" data-map-toggle="${esc(item.id)}" aria-expanded="${item.expanded}" aria-label="${item.expanded ? "Collapse" : "Expand"} ${esc(title)}">${item.expanded ? "−" : item.childCount}</button>` : ""}`;
        let card = existing;
        if (!card) {
          card = document.createElement("div");
          this.cards.set(item.id, card);
          this.layer.append(card);
        }
        card.className = `map-item${item.project ? " map-project" : ""}${item.id === this.selected ? " selected" : ""}${this.hits?.has(item.id) ? " search-hit" : ""}${item.id === this.currentHit ? " search-current" : ""}`;
        card.style.cssText = `left:${item.x}px;top:${item.y}px;--branch:${item.color}`;
        if (card.dataset.content !== html) {
          const active = card.contains(document.activeElement)
              ? document.activeElement
              : null,
            focus = active?.dataset.mapToggle
              ? "[data-map-toggle]"
              : active?.dataset.mapNode
                ? "[data-map-node]"
                : null;
          card.innerHTML = html;
          card.dataset.content = html;
          if (focus) card.querySelector(focus)?.focus({ preventScroll: true });
        }
      }
      for (const [id, card] of this.cards)
        if (!keep.has(id)) {
          card.remove();
          this.cards.delete(id);
        }
      if (this.selected)
        for (const link of this.links || [])
          if (link.from === this.selected || link.to === this.selected) {
            const a = d.positions.get(link.from),
              b = d.positions.get(link.to);
            if (a && b)
              curve(
                a,
                b,
                "var(--accent)",
                true,
                `${displayTitle(a)} → ${displayTitle(b)}: ${link.kind}${link.description ? ` — ${link.description}` : ""}`,
              );
          }
      this.edges.innerHTML =
        '<defs><marker id="map-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="5" markerHeight="5" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="var(--accent)" stroke="none"/></marker></defs>' +
        paths.join("");
      const zoom = document.getElementById("map-zoom");
      if (zoom) zoom.textContent = `${Math.round(c.scale * 100)}%`;
      if (this.overview && this.overviewCache) {
        const ctx = this.overview.querySelector("canvas").getContext("2d"),
          sx = 160 / d.width,
          sy = 96 / d.height;
        ctx.clearRect(0, 0, 160, 96);
        ctx.drawImage(this.overviewCache, 0, 0);
        ctx.strokeStyle = getComputedStyle(this.element).getPropertyValue(
          "--accent",
        );
        ctx.lineWidth = 1.5;
        ctx.strokeRect(
          Math.max(1, view.x * sx),
          Math.max(1, view.y * sy),
          Math.min(158, Math.max(3, view.width * sx)),
          Math.min(94, Math.max(3, view.height * sy)),
        );
      }
    }
  }
  const api = { layout, inViewport, indexGraph, Mindmap, displayTitle, kindIcon };
  if (typeof module !== "undefined") module.exports = api;
  else root.HeyBossMap = api;
})(typeof window !== "undefined" ? window : globalThis);
