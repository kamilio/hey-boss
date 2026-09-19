const day=date=>new Date(date.getFullYear(),date.getMonth(),date.getDate()).getTime();
export function activityGroups(tasks,now=new Date()){
 const today=day(now),yesterday=new Date(now.getFullYear(),now.getMonth(),now.getDate()-1).getTime();
 const groups=new Map();
 for(const task of tasks){
  const date=task.completedAt?new Date(task.completedAt*1000):null;
  const valid=date&&Number.isFinite(date.getTime());const timestamp=valid?day(date):0;
  const label=!valid?'Earlier':timestamp===today?'Today':timestamp===yesterday?'Yesterday':date.toLocaleDateString(undefined,{month:'short',day:'numeric',...(date.getFullYear()!==now.getFullYear()?{year:'numeric'}:{})});
  if(!groups.has(label))groups.set(label,{label,timestamp,tasks:[]});groups.get(label).tasks.push(task);
 }
 return [...groups.values()].sort((a,b)=>b.timestamp-a.timestamp);
}
const completedDate=task=>{const date=task.completedAt?new Date(task.completedAt*1000):null;return date&&Number.isFinite(date.getTime())?date:null;};
export function activityTime(task){return completedDate(task)?.toLocaleTimeString(undefined,{hour:'numeric',minute:'2-digit'})??'';}
export function activityDateTime(task){return completedDate(task)?.toISOString();}
