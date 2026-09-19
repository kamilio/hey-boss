import {unified} from 'unified';
import remarkParse from 'remark-parse';
import remarkGfm from 'remark-gfm';

const parser=unified().use(remarkParse).use(remarkGfm);
function text(node){
 const children=()=>node.children?.map(text).join('')??'';
 switch(node.type){
  case 'html':return '';
  case 'text':case 'inlineCode':case 'code':return node.value;
  case 'image':case 'imageReference':return node.alt??'';
  case 'break':return '\n';
  case 'root':return node.children.map(text).join('\n\n');
  case 'list':return node.children.map((item,i)=>(node.ordered?`${(node.start??1)+i}. `:'• ')+text(item)).join('\n');
  case 'listItem':return (node.checked===true?'✓ ':node.checked===false?'☐ ':'')+node.children.map(text).join('\n');
  case 'table':return node.children.map(text).join('\n');
  case 'tableRow':return node.children.map(text).join(' · ');
  case 'thematicBreak':return '';
  default:return children();
 }
}
export function markdownText(value){return text(parser.parse(String(value??''))).replace(/[\t ]+/g,' ').replace(/\n{3,}/g,'\n\n').trim();}
export function preview(value,max=240){
 const clean=markdownText(String(value??'').slice(0,8192));const chars=[...new Intl.Segmenter(undefined,{granularity:'grapheme'}).segment(clean)].map(x=>x.segment);
 return chars.length<=max?clean:chars.slice(0,max-1).join('').trimEnd()+'…';
}
export function pushContent(task){return {title:preview(task.title,90),body:preview(task.kind==='update'?task.description||task.question:task.question||task.description,240)};}
