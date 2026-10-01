// Isolated settings UI with deterministic API responses; no real project mutations.
import {createServer} from 'node:http';
import {readFileSync, readdirSync} from 'node:fs';
const source = new URL('../src/issues/web/', import.meta.url);
const index = readFileSync(new URL('index.html', source), 'utf8');
const dialog = index.match(/<dialog\s+id="project-settings-dialog"[^]*?<\/dialog>/)[0];
const prompts = new URL('../src/issues/prompts/', import.meta.url);
const promptDefaults = Object.fromEntries(readdirSync(prompts).filter(name => name.endsWith('.md') && !['worker.md', 'chief.md'].includes(name)).map(name => [name.slice(0, -3), readFileSync(new URL(name, prompts), 'utf8').trimEnd()]));
promptDefaults.chief_wrapper = 'Chief for {{project}}: {{prompt}}';
const server = createServer((req, res) => {
  if (/^\/[\w-]+\.(css|js)$/.test(req.url)) {
    try {
      res.setHeader('Content-Type', req.url.endsWith('.css') ? 'text/css; charset=utf-8' : 'text/javascript; charset=utf-8');
      res.end(readFileSync(new URL(req.url.slice(1), source)));
    } catch { res.writeHead(404).end(); }
    return;
  }
  res.setHeader('Content-Type', 'text/html; charset=utf-8');
  res.end(`<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
    <link rel="stylesheet" href="/components.css"><link rel="stylesheet" href="/app.css"><link rel="stylesheet" href="/project-settings.css">
    <button id="project-settings-trigger">Project settings</button>${dialog}
    <script>
      const $ = selector => document.querySelector(selector);
      const model = {project:{id:'fixture',name:'Settings fixture'}};
      const detailCache = new Map();
      const renderRoute = async () => {}, updateHeader = () => {}, toast = () => {};
      window.saved = []; window.failSave = false; window.failLoad = false;
      window.fixtureSettings = {version:1,prompt:'Fixture implementation',chief_prompt:'Fixture chief',
        chief_default_prompt:'Fixture chief',chief_preview_template:'Chief for {{project}}: {{prompt}}',chief_enabled:false,prs_enabled:false,worktree_enabled:true,
        drafts_enabled:true,plan_template:'plans/{number}.md',
        prompt_defaults:${JSON.stringify(promptDefaults)},prompt_sections:${JSON.stringify(Object.keys(promptDefaults).map(key => ({key,title:key,help:'Fixture prompt section'})))},prompt_overrides:{}};
      async function api(request) {
        if (request.action === 'project_settings') {
          if (window.failLoad) throw Error('Fixture load failed');
          return {...window.fixtureSettings};
        }
        window.preview = request;
        return {prompt:request.task_kind === 'plan' ? request.config.prompt_overrides.plan : request.config.prompt,use_goal:false};
      }
      async function mutate(request) {
        if (window.failSave) throw Error('Fixture save failed');
        window.saved.push(request);
        Object.assign(window.fixtureSettings,request);
        return {version:2};
      }
    </script><script src="/project-settings.js"></script></html>`);
});
server.listen(59642, '127.0.0.1', () => console.log('Settings fixture: http://127.0.0.1:59642'));
