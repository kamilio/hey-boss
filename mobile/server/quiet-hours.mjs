const clocks=new Map();
export function validQuietHours(value){
 if(!value||typeof value.enabled!=='boolean'||!/^([01]\d|2[0-3]):[0-5]\d$/.test(value.start)||!/^([01]\d|2[0-3]):[0-5]\d$/.test(value.end)||value.start===value.end||typeof value.time_zone!=='string')return false;
 try{new Intl.DateTimeFormat('en',{timeZone:value.time_zone});return true;}catch{return false;}
}
export function quietHoursActive(schedule,now){
 if(!schedule?.enabled)return false;
 let clock=clocks.get(schedule.time_zone);
 if(!clock){clock=new Intl.DateTimeFormat('en-GB',{timeZone:schedule.time_zone,hour:'2-digit',minute:'2-digit',hourCycle:'h23'});if(clocks.size>=16)clocks.clear();clocks.set(schedule.time_zone,clock);}
 const time=clock.format(new Date(now));
 return schedule.start<schedule.end?time>=schedule.start&&time<schedule.end:time>=schedule.start||time<schedule.end;
}
