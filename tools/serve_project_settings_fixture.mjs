// Isolated settings UI with deterministic API responses; no real project mutations.
import {createServer} from 'node:http';
import {readFileSync} from 'node:fs';
const source = new URL('../src/issues/web/', import.meta.url);
const index = readFileSync(new URL('index.html', source), 'utf8');
const dialog = index.match(/<dialog\s+id="project-settings-dialog"[^]*?<\/dialog>/)[0];
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
      const refreshProjects = async () => {}, renderRoute = async () => {}, updateHeader = () => {}, toast = () => {};
      window.saved = []; window.failSave = false; window.failLoad = false;
      window.fixtureSettings = {version:1,prs_enabled:false,worktree_enabled:true,
        drafts_enabled:true,artifacts_enabled:true,
        prompt_overrides:{}};
      async function api(request) {
        if (request.action === 'project_config') return {revisions:[],working_copies:[],next_offset:null};
        if (request.action === 'project_settings') {
          if (window.failLoad) throw Error('Fixture load failed');
          return {...window.fixtureSettings};
        }
        throw Error('Unexpected settings operation');
      }
      async function mutate(request) {
        if (window.failSave) throw Error('Fixture save failed');
        window.saved.push(request);
        for (const [key,value] of Object.entries(request.operation.changes)) {
          if (key.startsWith('prompt_overrides.')) window.fixtureSettings.prompt_overrides[key.slice(17)] = value;
          else window.fixtureSettings[key] = value;
        }
        return {version:2};
      }
    </script><script src="/project-settings.js"></script></html>`);
});
const port = Number(process.env.HEY_BOSS_SETTINGS_FIXTURE_PORT || 59642);
server.listen(port, '127.0.0.1', () => console.log(`Settings fixture: http://127.0.0.1:${port}`));
