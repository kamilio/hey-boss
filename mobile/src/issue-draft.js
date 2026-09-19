const key='hey-boss-issue-draft';
export const emptyIssueDraft=()=>({project:'',title:'',body:'',labels:'',requestID:crypto.randomUUID(),submitted:false});
export function loadIssueDraft(storage){try{const draft=JSON.parse(storage.getItem(key));if(draft&&['project','title','body','labels','requestID'].every(k=>typeof draft[k]==='string'))return draft;}catch{}return emptyIssueDraft();}
export function saveIssueDraft(storage,draft){storage.setItem(key,JSON.stringify(draft));}
export const issuePayload=draft=>({requestID:draft.requestID,project:draft.project,title:draft.title,body:draft.body,labels:draft.labels.split(',').map(label=>label.trim()).filter(Boolean)});
