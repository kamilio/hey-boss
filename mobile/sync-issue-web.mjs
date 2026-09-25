// Ship the regular issue interface, rather than maintain a second mobile editor.
import {readFileSync,writeFileSync,mkdirSync,readdirSync,cpSync} from 'node:fs';
import {addAgentGuidance} from './agent-guidance.mjs';
const source=new URL('../src/issues/web/',import.meta.url),destination=new URL('./public/issue-web/',import.meta.url);
mkdirSync(destination,{recursive:true});
cpSync(new URL('diagram-assets/',source),new URL('diagram-assets/',destination),{recursive:true});
let shell=readFileSync(new URL('app-shell.html',source),'utf8')
 .replace('<!--header-actions-->',readFileSync(new URL('issue-header-actions.html',source),'utf8'))
 .replace('<!--profile-->',readFileSync(new URL('issue-profile.html',source),'utf8'))
 .replace('<!--project-action-->',readFileSync(new URL('issue-project-action.html',source),'utf8'))
 .replace('<a id="nav-admin"','<a hidden id="nav-admin"')
 .replace('<a id="nav-workers"','<a hidden id="nav-workers"')
 .replace(/<!--[^]*?-->/g,'');
for(const name of readdirSync(source).filter(name=>/\.(js|css|png)$/.test(name)||['index.html','mindmap.html','artifacts.html'].includes(name))){
 let value=readFileSync(new URL(name,source));
 if(name==='routes.js')value=Buffer.from(value.toString().replace('/* ROUTE_DEFINITIONS */ []',readFileSync(new URL('routes.json',source),'utf8')));
 if(name.endsWith('.html')){
  value=Buffer.from(addAgentGuidance(value.toString().replace('<!--app-shell-->',shell)
   .replace('<html lang="en">','<html lang="en" data-issue-mobile="true">')
   .replace(/(src|href)="\/([^"/]+\.(?:js|css|png))"/g,'$1="/issue-web/$2"')
   .replace('</head>','<link rel="manifest" href="/manifest.webmanifest"><link rel="apple-touch-icon" href="/icons/apple-touch-icon.png"><link rel="stylesheet" href="/issue-web/mobile.css"><script src="/issue-web/mobile.js" defer></script></head>')));
 }
 writeFileSync(new URL(name,destination),value);
}
for(const name of ['mobile.css','mobile.js'])cpSync(new URL('./web/'+name,import.meta.url),new URL(name,destination));
