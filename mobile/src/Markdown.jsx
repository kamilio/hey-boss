import React,{useState,useEffect,useMemo,Fragment} from 'react';
import {jsx,jsxs} from 'react/jsx-runtime';
import {Spinner} from '@radix-ui/themes';
import {ArrowUpRight} from 'lucide-react';
import ReactMarkdown,{defaultUrlTransform} from 'react-markdown';
import remarkGfm from 'remark-gfm';
import {toJsxRuntime} from 'hast-util-to-jsx-runtime';

// Keep one semantic list/table in the DOM. Appending batches keeps long
// documents selectable and searchable without a single expensive DOM commit.
const Batch=React.memo(function Batch({items}){return <>{items}</>;});
function Progressive({tag:Tag,children,...props}){
 const batches=useMemo(()=>{const items=React.Children.toArray(children),result=[];for(let i=0;i<items.length;i+=80)result.push(items.slice(i,i+80));return result;},[children]);
 const [visible,setVisible]=useState(1);
 useEffect(()=>{if(visible>=batches.length)return;const frame=requestAnimationFrame(()=>setVisible(count=>count+1));return()=>cancelAnimationFrame(frame);},[visible,batches.length]);
 return <Tag {...props}>{batches.slice(0,visible).map((items,index)=><Batch key={index} items={items}/>)}</Tag>;
}
const components={
 a:({node,href,children,...props})=>!href?<span>{children}</span>:href.startsWith('#')?<a {...props} href={href}>{children}</a>:<a {...props} href={href} target="_blank" rel="noreferrer">{children}</a>,
 img:({alt,src})=><span className="image-reference">{/^https?:\/\//i.test(src??'')?<a href={src} target="_blank" rel="noreferrer">{alt||'View image'} <ArrowUpRight size={14}/></a>:alt||'Image'}</span>,
 table:({node,...props})=><div className="table-scroll"><table {...props}/></div>,
 ul:({node,...props})=><Progressive tag="ul" {...props}/>,
 ol:({node,...props})=><Progressive tag="ol" {...props}/>,
 tbody:({node,...props})=><Progressive tag="tbody" {...props}/>,
 blockquote:({node,...props})=><Progressive tag="blockquote" {...props}/>,
};
function safeTree(node){
 if(node.properties)for(const name of ['href','src','cite'])if(typeof node.properties[name]==='string')node.properties[name]=defaultUrlTransform(node.properties[name]);
 for(const child of node.children??[])safeTree(child);return node;
}
function WorkerDocument({source}){
 const [state,setState]=useState({source,tree:null,error:null});
 useEffect(()=>{
  const worker=new Worker(new URL('./markdown-worker.js',import.meta.url),{type:'module'});
  const fail=()=>setState({source,tree:null,error:'The document could not load. Try reopening it.'});
  worker.onmessage=event=>setState({source,tree:event.data.tree?safeTree(event.data.tree):null,error:event.data.error});worker.onerror=fail;
  worker.postMessage({id:1,source});return()=>worker.terminate();
 },[source]);
 const content=useMemo(()=>state.source===source&&state.tree?toJsxRuntime(state.tree,{jsx,jsxs,Fragment,components,passNode:true}):null,[state,source]);
 return content?<Progressive tag={Fragment}>{content.props.children}</Progressive>:state.source===source&&state.error?<p role="alert">{state.error}</p>:<div className="document-loading" role="status"><Spinner/>Loading document…</div>;
}
export default function Markdown({children}){
 const source=String(children??'');
 return <div className="markdown">{source.length>16000&&typeof Worker!=='undefined'?<WorkerDocument key={source} source={source}/>:<ReactMarkdown remarkPlugins={[remarkGfm]} skipHtml components={components}>{source}</ReactMarkdown>}</div>;
}
