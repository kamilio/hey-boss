import mermaid from 'mermaid';

const dark=matchMedia('(prefers-color-scheme: dark)');
let serial=0,queue=Promise.resolve();
const pending=new Set();
const observer=new IntersectionObserver(entries=>{
  for(const entry of entries)if(entry.isIntersecting){
    observer.unobserve(entry.target);pending.delete(entry.target);schedule(entry.target);
  }
},{rootMargin:'300px'});
new MutationObserver(()=>{
  for(const figure of pending)if(!figure.isConnected){observer.unobserve(figure);pending.delete(figure);}
}).observe(document.body,{childList:true,subtree:true});

function configure(){
  mermaid.initialize({startOnLoad:false,securityLevel:'strict',suppressErrorRendering:true,
    maxTextSize:50000,maxEdges:500,htmlLabels:false,
    secure:['securityLevel','startOnLoad','suppressErrorRendering','maxTextSize','maxEdges','htmlLabels','flowchart','theme','themeVariables','themeCSS'],
    theme:dark.matches?'dark':'default',fontFamily:'system-ui, sans-serif',
    flowchart:{htmlLabels:false,useMaxWidth:false}});
}
function schedule(figure){
  // Mermaid owns global configuration and temporary SVG IDs: serialize renders.
  queue=queue.catch(()=>{}).then(()=>render(figure));
}
async function render(figure){
  if(!figure.isConnected)return;
  const stage=figure.querySelector('.artifact-diagram-stage');
  const status=figure.querySelector('[role="status"]');
  figure.dataset.state='loading';status.textContent='Rendering diagram…';
  const container=document.createElement('div');
  container.className='artifact-diagram-rendering';document.body.append(container);
  try{
    configure();
    const result=await mermaid.render('artifact-diagram-'+(++serial),figure.querySelector('code').textContent,container);
    if(!figure.isConnected)return;
    // Strict Mermaid rendering sanitizes the SVG and disables links/callbacks.
    const template=document.createElement('template');template.innerHTML=result.svg;
    const svg=template.content.querySelector('svg');
    if(!svg)throw Error('Missing diagram');
    svg.setAttribute('role','img');
    if(!svg.querySelector('title')){
      const title=document.createElementNS('http://www.w3.org/2000/svg','title');
      title.id=svg.id+'-title';title.textContent='Mermaid diagram';svg.prepend(title);svg.setAttribute('aria-labelledby',title.id);
    }
    stage.replaceChildren(svg);figure.dataset.state='ready';status.textContent='';
    figure.querySelector('[data-expand]').disabled=false;
  }catch(e){
    if(!figure.isConnected)return;
    stage.replaceChildren();figure.dataset.state='error';status.textContent='This diagram could not render. Check the Mermaid source below.';
    figure.querySelector('details').open=true;figure.querySelector('[data-expand]').disabled=true;
  }finally{container.remove();}
}

function expand(figure){
  const original=figure.querySelector('.artifact-diagram-stage svg');
  if(!original)return;
  const dialog=document.createElement('dialog');dialog.className='artifact-diagram-dialog';
  dialog.setAttribute('aria-label','Expanded diagram');
  dialog.innerHTML='<header><strong>Diagram</strong><div class="artifact-diagram-controls"><button class="button" type="button" data-out aria-label="Zoom out">−</button><output aria-label="Zoom level">100%</output><button class="button" type="button" data-in aria-label="Zoom in">+</button><button class="button" type="button" data-fit>Fit</button><button class="button" type="button" data-actual aria-label="Actual size">100%</button><button class="button" type="button" data-close aria-label="Close diagram">Close</button></div></header><div class="artifact-diagram-viewport" tabindex="0" role="region" aria-label="Diagram; scroll to explore"></div>';
  // Move, rather than clone, so SVG marker and title IDs remain unique.
  const viewport=dialog.querySelector('.artifact-diagram-viewport');viewport.append(original);
  let scale=1,fitted=true;
  const box=original.viewBox.baseVal;
  function zoom(next){
    scale=Math.max(.02,Math.min(4,next));original.style.width=(box.width*scale)+'px';original.style.height='auto';
    dialog.querySelector('output').textContent=Math.round(scale*100)+'%';
    dialog.querySelector('[data-out]').disabled=scale<=.02;dialog.querySelector('[data-in]').disabled=scale>=4;
  }
  function fit(){
    fitted=true;zoom(Math.min(1,(viewport.clientWidth-32)/box.width,(viewport.clientHeight-32)/box.height));
    viewport.scrollTo(0,0);
  }
  dialog.querySelector('[data-in]').onclick=()=>{fitted=false;zoom(scale+.25);};
  dialog.querySelector('[data-out]').onclick=()=>{fitted=false;zoom(scale-.25);};
  dialog.querySelector('[data-fit]').onclick=fit;
  dialog.querySelector('[data-actual]').onclick=()=>{fitted=false;zoom(1);};
  dialog.querySelector('[data-close]').onclick=()=>dialog.close();
  dialog.addEventListener('click',e=>{if(e.target===dialog)dialog.close();});
  const resize=new ResizeObserver(()=>{if(fitted)fit();});
  const removed=new MutationObserver(()=>{if(!figure.isConnected)dialog.close();});
  dialog.addEventListener('close',()=>{
    resize.disconnect();removed.disconnect();original.style.removeProperty('width');original.style.removeProperty('height');
    if(figure.isConnected){figure.querySelector('.artifact-diagram-stage').append(original);figure.querySelector('[data-expand]').focus();}
    dialog.remove();
  },{once:true});
  document.body.append(dialog);dialog.showModal();fit();resize.observe(viewport);
  removed.observe(document.body,{childList:true,subtree:true});
}

function mount(root){
  for(const code of root.querySelectorAll('pre > code.language-mermaid')){
    if(code.closest('.artifact-diagram'))continue;
    const pre=code.parentElement,figure=document.createElement('figure');figure.className='artifact-diagram';figure.dataset.state='waiting';
    figure.innerHTML='<figcaption><span>Diagram</span><button class="artifact-text-button" type="button" data-expand disabled>Expand diagram</button></figcaption><div class="artifact-diagram-stage" tabindex="0" role="region" aria-label="Diagram; scroll to explore"></div><p class="artifact-diagram-status" role="status">Diagram renders when visible.</p><details><summary>Mermaid source</summary></details>';
    pre.replaceWith(figure);figure.querySelector('details').append(pre);
    figure.querySelector('[data-expand]').onclick=()=>expand(figure);
    pending.add(figure);observer.observe(figure);
  }
}
dark.addEventListener('change',()=>{
  document.querySelector('.artifact-diagram-dialog')?.close();
  for(const figure of document.querySelectorAll('.artifact-diagram[data-state="ready"]'))schedule(figure);
});
window.HeyBossArtifactDiagrams={mount};
