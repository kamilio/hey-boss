const key='hey-boss-issue-draft';
export const taskKind=labels=>labels.includes('task:research')?'research':labels.includes('task:plan')?'plan':'implement';
export const taskLabels=(labels,kind)=>[...new Set(labels.filter(label=>!['task:plan','task:research'].includes(label))),...(['plan','research'].includes(kind)?[`task:${kind}`]:[])];
export const emptyIssueDraft=()=>({project:'',title:'',body:'',labels:'',task:'implement',requestID:crypto.randomUUID(),submitted:false});
export function loadIssueDraft(storage){try{const draft=JSON.parse(storage.getItem(key));if(draft&&['project','title','body','labels','requestID'].every(k=>typeof draft[k]==='string'))return draft;}catch{}return emptyIssueDraft();}
export function saveIssueDraft(storage,draft){storage.setItem(key,JSON.stringify(draft));}
export const issuePayload=draft=>{
 const labels=draft.labels.split(',').map(label=>label.trim()).filter(Boolean);
 return {requestID:draft.requestID,project:draft.project,title:draft.title,body:draft.body,labels:draft.task===undefined?labels:taskLabels(labels,draft.task)};
};
