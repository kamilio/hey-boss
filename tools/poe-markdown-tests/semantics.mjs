// Compare document meaning independently of ANSI/HTML/AppKit presentation.
export function semantic(node) {
  if(!node || node.type==='frontmatter' || node.type==='task') return [];
  const children=(node.children??[]).flatMap(semantic);
  const merged=[];
  for(const child of children) {
    if(child.type==='text' && merged.at(-1)?.type==='text') merged.at(-1).value+=child.value;
    else merged.push(child);
  }
  let type=node.type;
  if(type==='container') return merged;
  if(['text','html','softBreak','break'].includes(type)) return [{type:'text',value:node.value??'\n'}];
  if(type==='listItem') {
    const inline=new Set(['text','emphasis','strong','strikethrough','inlineCode','link','image','footnoteReference']);
    const blocks=[];
    for(const child of merged) {
      if(inline.has(child.type)) {
        if(blocks.at(-1)?.type!=='paragraph') blocks.push({type:'paragraph',children:[]});
        blocks.at(-1).children.push(child);
      } else blocks.push(child);
    }
    merged.splice(0,merged.length,...blocks);
  }
  const result={type};
  if(['code','inlineCode'].includes(type)) result.value=type==='code'?node.value.replace(/\n$/,''):node.value;
  if(type==='code') result.lang=(node.lang??'').split(/\s+/)[0];
  if(type==='heading') result.depth=node.depth;
  if(type==='list') result.start=node.start??(node.ordered?1:0);
  if(type==='listItem') {
    const task=(n)=>n.type==='task'?n.checked:n.type==='list'?undefined:(n.children??[]).map(task).find(x=>x!==undefined);
    result.checked=node.checked??task(node)??null;
  }
  if(type==='alert') result.kind=(node.kind??node.value).toUpperCase();
  if(type==='table') result.align=node.align.map(x=>x??'left');
  if(type==='link' || type==='image') result.url=node.url??'';
  if(type==='image') result.alt=node.alt??merged.map(n=>n.value??'').join('');
  if(type.startsWith('footnote')) result.label=node.label??node.value;
  if(node.children && type!=='image') result.children=merged;
  // Empty nodes serialize identically regardless of omitted empty arrays.
  if(!['text','code','inlineCode','image','thematicBreak','footnoteReference'].includes(type)) result.children??=[];
  return [result];
}
export function canonical(ast) {
  const result=semantic(ast)[0]??{type:'root',children:[]};
  function normalize(n) {
    if(n.type==='text') n.value=n.value.replace(/\s+/g,' ');
    for(const child of n.children??[]) normalize(child);
    if(n.type==='root') for(const child of n.children??[]) if(child.type==='text') child.value=child.value.trimEnd();
  }
  normalize(result);return result;
}
