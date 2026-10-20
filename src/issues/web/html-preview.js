"use strict";
const HeyBossHTML = (() => {
  const previews = new Set();
  const dark = matchMedia("(prefers-color-scheme: dark)");
  const channel = "hey-boss-html";
  let serial = 0;
  function theme(root) {
    const css = getComputedStyle(root), global = getComputedStyle(document.documentElement);
    const color = (name, fallback) => global.getPropertyValue(name).trim() || fallback;
    return {dark: global.colorScheme === "dark" || (global.colorScheme.includes("dark") && dark.matches), variables:{
      "--background":color("--bg", "transparent"), "--foreground":css.color,
      "--muted":color("--hover", "#80808012"), "--muted-foreground":color("--muted", "#808080"),
      "--card":color("--solid", "#fff"), "--card-foreground":css.color,
      "--border":color("--line", "#80808030"), "--input":color("--input", "#80808012"),
      "--primary":color("--primary-bg", color("--accent", "#4655b5")), "--primary-foreground":color("--primary-text", "#fff"),
      "--accent":color("--accent", "#4655b5"), "--accent-foreground":css.color,
      "--accent-surface":color("--accent-soft", "#5367c313"), "--ring":color("--accent", "#4655b5"),
      "--destructive":color("--danger", "#b64559"), "--success":color("--green", "#20745d"), "--warning":color("--warning", "#af740e"),
      "--chart-1":color("--accent", "#4655b5"), "--chart-2":color("--green", "#20745d"), "--chart-3":color("--warning", "#af740e"), "--chart-4":color("--purple", "#8560b0"), "--chart-5":color("--danger", "#b64559"), "--chart-6":color("--active", "#168179"),
      "--radius":"10px", "--font-sans":css.fontFamily, "--font-mono":"ui-monospace, SFMono-Regular, Menlo, monospace"
    }};
  }
  function send(item, type, extra = {}) {
    item.frame?.contentWindow?.postMessage({channel,type,...extra}, "*");
  }
  function failed(item, message) {
    if (!item.figure.isConnected) return;
    item.status.textContent = message;
    item.status.hidden = false;
    item.figure.dataset.state = "error";
    item.source.open = true;
    clearTimeout(item.timer);
  }
  function load(item) {
    observer.unobserve(item.figure);
    if (!item.figure.isConnected || item.frame) return;
    const frame = document.createElement("iframe");
    item.frame = frame;
    frame.title = "Interactive HTML preview";
    frame.setAttribute("sandbox", "allow-scripts");
    frame.setAttribute("referrerpolicy", "no-referrer");
    frame.setAttribute("scrolling", "no");
    frame.setAttribute("allow", "camera 'none'; microphone 'none'; geolocation 'none'; clipboard-read 'none'; clipboard-write 'none'");
    frame.src = new URL("/html-preview", document.baseURI).href;
    let loads = 0;
    frame.addEventListener("load", () => {
      if (++loads === 1) {
        send(item,"init",{html:item.code.textContent,theme:theme(item.figure)});
      } else if (loads > 2) {
        // Navigation ends a preview's authority to send messages to the host.
        frame.remove(); item.frame = null;
        failed(item,"This preview left its document. The HTML source is shown below.");
      }
    });
    item.timer = setTimeout(() => failed(item,"The preview could not load. The HTML source is shown below."), 10000);
    item.stage.append(frame);
    item.figure.dataset.state = "loading";
    item.status.textContent = "Loading preview…";
  }
  const observer = new IntersectionObserver(entries => {
    for (const entry of entries) if (entry.isIntersecting) {
      const item = [...previews].find(item => item.figure === entry.target);
      if (item) load(item);
    }
  }, {rootMargin:"300px"});
  function mount(root) {
    for (const code of root.querySelectorAll(".markdown pre > code.language-html-render, pre > code.language-html-render")) {
      if (code.closest(".html-preview") || !code.closest(".markdown")) continue;
      const pre = code.parentElement, figure = document.createElement("figure");
      figure.className = "html-preview"; figure.dataset.state = "waiting";
      figure.innerHTML = '<figcaption><span>HTML preview</span><div><button type="button" data-expand disabled>Expand</button></div></figcaption><dialog open class="html-preview-stage" role="presentation"><div class="html-preview-fullscreen-bar"><strong>HTML preview</strong><button type="button" data-close aria-label="Close HTML preview">Close <kbd>Esc</kbd></button></div></dialog><p class="html-preview-status" role="status">Preview loads when visible.</p><details class="html-preview-source"><summary>HTML source</summary></details>';
      pre.replaceWith(figure);
      const source = figure.querySelector("details"), stage = figure.querySelector("dialog"), expand = figure.querySelector("[data-expand]");
      source.append(pre);
      stage.id = "html-preview-" + (++serial); expand.setAttribute("aria-controls",stage.id);
      const item = {figure,code,source,stage,expand,status:figure.querySelector("[role=status]"),frame:null,ready:false};
      previews.add(item); observer.observe(figure);
      const close = () => { if (stage.matches(":modal")) stage.close(); };
      expand.addEventListener("click", () => {
        stage.close(); stage.setAttribute("role","dialog"); stage.setAttribute("aria-label","Expanded HTML preview"); stage.showModal();
      });
      stage.querySelector("[data-close]").addEventListener("click",close);
      stage.addEventListener("cancel", event => {event.preventDefault();close();});
      stage.addEventListener("close", () => {
        if (stage.matches(":modal")) return;
        stage.setAttribute("open",""); stage.setAttribute("role","presentation"); stage.removeAttribute("aria-label");
        if (figure.isConnected) expand.focus({preventScroll:true});
      });
    }
  }
  addEventListener("message", event => {
    const data = event.data;
    if (data?.channel !== channel) return;
    const item = [...previews].find(item => item.frame?.contentWindow === event.source);
    if (!item || !item.figure.isConnected) return;
    if (data.type === "ready") {
      item.ready = true; clearTimeout(item.timer); item.expand.disabled = false;
      if (item.figure.dataset.state !== "error") {item.figure.dataset.state="ready";item.status.hidden=true;}
    } else if (data.type === "size" && Number.isFinite(data.height) && data.height > 0) {
      item.frame.style.height = Math.max(1,Math.ceil(data.height)) + "px";
    } else if (data.type === "error") {
      failed(item,typeof data.message === "string" ? data.message.slice(0,180) : "The preview could not run. Check its HTML source.");
    } else if (data.type === "escape") {
      if (item.stage.matches(":modal")) item.stage.close();
    } else if (data.type === "link" && document.activeElement === item.frame && navigator.userActivation?.isActive && typeof data.url === "string" && /^https?:\/\//i.test(data.url)) {
      window.open(data.url,"_blank","noopener,noreferrer");
    }
  });
  function refreshTheme() {for (const item of previews) send(item,"theme",{theme:theme(item.figure)});}
  // Reparenting even the same iframe reloads its browsing context. Replace only
  // the siblings around unchanged Markdown, keeping its entire ancestor path
  // connected when a comment or sidebar changes.
  function replaceContent(root, html, keep) {
    if (!keep || !root.contains(keep)) {root.innerHTML=html;return;}
    const template=document.createElement("template");template.innerHTML=html;
    let current=keep,next=template.content.querySelector("#"+CSS.escape(keep.id));
    if (!next) {root.innerHTML=html;return;}
    while (next) {
      const parent=current.parentNode,target=next.parentNode;
      const before=[],after=[];let passed=false;
      for (const node of target.childNodes) {
        if (node===next) {passed=true;continue;}
        (passed?after:before).push(node);
      }
      for (const node of [...parent.childNodes]) if(node!==current)node.remove();
      current.before(...before);current.after(...after);
      if (parent===root) return;
      for (const attribute of [...parent.attributes]) if(!target.hasAttribute(attribute.name))parent.removeAttribute(attribute.name);
      for (const attribute of target.attributes) parent.setAttribute(attribute.name,attribute.value);
      current=parent;next=target;
    }
  }
  dark.addEventListener("change",refreshTheme);
  new MutationObserver(refreshTheme).observe(document.documentElement,{attributes:true,attributeFilter:["class","style","data-theme"]});
  let queued = false;
  const mutations = new MutationObserver(records => {
    for (const item of previews) if (!item.figure.isConnected) {
      observer.unobserve(item.figure); clearTimeout(item.timer); if (item.stage.matches(":modal")) item.stage.close(); previews.delete(item);
    }
    if (queued || !records.some(record => [...record.addedNodes].some(node => node.nodeType === 1 && (node.matches?.(".markdown,pre,code") || node.querySelector?.(".markdown,pre > code.language-html-render"))))) return;
    queued = true; requestAnimationFrame(() => {queued=false;mount(document);});
  });
  function start() {
    if (location.protocol === "hey-boss-preview:") {
      for (const input of document.querySelectorAll('input[type="checkbox"][data-task-line]')) {
        input.disabled = false;
        input.addEventListener("change", () => window.webkit?.messageHandlers?.artifactTask?.postMessage({line:Number(input.dataset.taskLine),checked:input.checked}));
      }
      for (const image of document.querySelectorAll("article img[src]")) {
        const source = image.getAttribute("src");
        if (source && !source.startsWith("data:")) image.src = new URL("/image?source=" + encodeURIComponent(source),document.baseURI).href;
      }
    }
    mount(document);mutations.observe(document.body,{childList:true,subtree:true});
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded",start,{once:true}); else start();
  return {mount,replaceContent};
})();
