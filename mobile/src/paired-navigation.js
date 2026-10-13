// Keep the requested page in the address bar while pairing. Destinations are
// local paths; no return URL or persisted copy of private link data is needed.
export function pairedDestination({pathname,search,hash}){
 if(['/issues','/jobs','/merged-prs','/mm','/artifacts','/project-resource','/agents','/agents/session','/workers'].includes(pathname))return pathname+search+hash;
 const route=new URLSearchParams(hash.slice(1));
 if(pathname==='/'&&(hash==='#issues'||route.has('issue')||route.get('view')==='issues'))return '/issues'+search+(hash==='#issues'?'':hash);
 return null;
}
