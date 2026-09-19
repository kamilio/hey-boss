import {unified} from 'unified';
import remarkParse from 'remark-parse';
import remarkGfm from 'remark-gfm';
import remarkRehype from 'remark-rehype';

const parser=unified().use(remarkParse).use(remarkGfm).use(remarkRehype);
function compact(node){delete node.position;for(const child of node.children??[])compact(child);}
export async function parseMarkdown(source){const tree=await parser.run(parser.parse(source));compact(tree);return tree;}
if(typeof self!=='undefined')self.onmessage=async event=>{
 const {id,source}=event.data;
 try{const tree=await parseMarkdown(source);self.postMessage({id,tree});}
 catch{self.postMessage({id,error:'This document could not be displayed.'});}
};
