import {readFileSync,writeFileSync,mkdirSync,cpSync,rmSync} from 'node:fs';
const source=new URL('../src/issues/web/',import.meta.url);
const destination=new URL('./public/artifact-web/',import.meta.url);
mkdirSync(destination,{recursive:true});
rmSync(new URL('diagram-assets/',destination),{recursive:true,force:true});
cpSync(new URL('diagram-assets/',source),new URL('diagram-assets/',destination),{recursive:true});
for(const name of ['artifacts.html','artifact-editor.js','artifact-diagrams.js','artifacts.js','artifacts.css','components.js','components.css','app.css','icon.png']){
 let value=readFileSync(new URL(name,source));
 if(name==='artifacts.html'){
  let html=value.toString().replace('<!--app-shell-->',readFileSync(new URL('app-shell.html',source),'utf8').replace(/<!--[^]*?-->/g,''));
  html=html.replace('<html lang="en">','<html lang="en" data-artifact-mobile="true">').replace(/(?:<script src="\/quick-issue.js" defer><\/script>)/g,'').replace(/(src|href)="\/(icon.png|components.js|components.css|app.css|artifacts.js|artifacts.css)"/g,'$1="/artifact-web/$2"');
  value=Buffer.from(html);
 }
 writeFileSync(new URL(name,destination),value);
}

const artifact=readFileSync(new URL('artifacts.html',destination),'utf8');
const start=artifact.indexOf('<main id="artifact-main"');
const end=artifact.indexOf('</main>',start)+7;
writeFileSync(new URL('resource.html',destination),artifact.slice(0,start)+'<main id="resource-main"><p id="resource-status" role="status">Loading project resource…</p><div id="resource-content"></div><section id="resource-artifacts"></section></main>'+artifact.slice(end));
