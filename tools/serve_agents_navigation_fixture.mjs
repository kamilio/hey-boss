// Read-only fixture for Agents navigation; no native services or agents required.
import {createServer} from 'node:http';
import {readFile} from 'node:fs/promises';
const root = new URL('../src/issues/web/', import.meta.url);
const projects = [{id:'named:Home',name:'Home'},{id:'named:Atlas',name:'Atlas'},{id:'named:Beacon',name:'Beacon'}];
createServer(async (req,res) => {
  res.setHeader('Cache-Control','no-store');
  const path = new URL(req.url,'http://fixture').pathname;
  const json = value => {res.setHeader('Content-Type','application/json');res.end(JSON.stringify(value));};
  if (path === '/api/bootstrap' || path === '/api/agent-bootstrap') return json({csrf:'fixture',project:projects[0],projects});
  if (path === '/api/fleet/status') return json({ok:true,machines:[],signals:[],conflicts:[]});
  if (path.startsWith('/api/')) {res.writeHead(404);return res.end();}
  try {
    const file = ['/agents','/workers','/agents/session'].includes(path) ? 'fleet.html' : path.slice(1);
    if (!/^[a-z-]+\.(html|js|css|png|json)$/.test(file)) throw Error('Not found');
    let body = await readFile(new URL(file,root));
    if (file === 'fleet.html') body = body.toString().replace('<!--app-shell-->',await readFile(new URL('app-shell.html',root),'utf8'));
    if (file === 'routes.js') body = body.toString().replace('/* ROUTE_DEFINITIONS */ []',await readFile(new URL('routes.json',root),'utf8'));
    res.setHeader('Content-Type',file.endsWith('.html')?'text/html':file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':'image/png');res.end(body);
  } catch {res.writeHead(404);res.end();}
}).listen(59643,'127.0.0.1',()=>console.log('Agents navigation fixture: http://127.0.0.1:59643'));
