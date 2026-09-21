const key='hey-boss-issue-draft';
export const taskKind=labels=>labels.some(label=>['task:plan','task:research'].includes(label))?'plan':'implement';
export const taskLabels=(labels,kind)=>[...new Set(labels.filter(label=>!['task:plan','task:research'].includes(label))),...(kind==='plan'?[`task:${kind}`]:[])];
export const emptyIssueDraft=()=>({project:'',title:'',body:'',labels:'',task:'implement',requestID:crypto.randomUUID(),submitted:false});
export function loadIssueDraft(storage){try{const draft=JSON.parse(storage.getItem(key));if(draft&&['project','title','body','labels','requestID'].every(k=>typeof draft[k]==='string'))return draft;}catch{}return emptyIssueDraft();}
export function saveIssueDraft(storage,draft){storage.setItem(key,JSON.stringify(draft));}
export const issuePayload=draft=>{
 const labels=draft.labels.split(',').map(label=>label.trim()).filter(Boolean);
 // A submitted Research draft must retry the exact pre-upgrade payload.
 const payloadLabels=draft.submitted&&draft.task==='research'?[...new Set(labels.filter(label=>!['task:plan','task:research'].includes(label))),'task:research']:draft.task===undefined?labels:taskLabels(labels,draft.task==='research'?'plan':draft.task);
 return {requestID:draft.requestID,project:draft.project,title:draft.title,body:draft.body,labels:payloadLabels};
};
