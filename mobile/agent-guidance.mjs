import {readFileSync} from 'node:fs';
export const guide=readFileSync(new URL('../src/issues/web/agent-guide.md',import.meta.url),'utf8');
export const script=readFileSync(new URL('../src/issues/web/agent-guide.js',import.meta.url),'utf8');
const head=readFileSync(new URL('../src/issues/web/agent-guide-head.html',import.meta.url),'utf8');
export function addAgentGuidance(html){
 const escaped=guide.replaceAll('&','&amp;').replaceAll('<','&lt;').replaceAll('>','&gt;');
 return html.replace('</head>',`${head}</head>`)
  .replace('</body>',`<pre hidden id="hey-boss-agent-guide">${escaped}</pre>\n</body>`);
}
export function agentGuidancePlugin(){
 return {name:'hey-boss-agent-guidance',transformIndexHtml:addAgentGuidance,
  generateBundle(){
   this.emitFile({type:'asset',fileName:'llms.txt',source:guide});
   this.emitFile({type:'asset',fileName:'agent-guide.js',source:script});
  }};
}
