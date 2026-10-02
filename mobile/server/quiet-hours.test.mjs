import {test} from 'node:test';
import assert from 'node:assert/strict';
import {HubStore} from './store.mjs';
import {createApp} from './index.mjs';
const schedule={enabled:true,start:'22:00',end:'07:00',time_zone:'America/Chicago'};
test('quiet hours use the selected time zone, boundaries, daytime ranges and DST',()=>{
 const store=new HubStore();
 try {
  store.setQuietHours(schedule);
  store.setPreferences({mode:'always',awayAfterSeconds:60});
  for(const [date,muted] of [['2026-10-02T02:59:59Z',false],['2026-10-02T03:00:00Z',true],['2026-10-02T11:59:59Z',true],['2026-10-02T12:00:00Z',false],['2026-11-01T06:30:00Z',true],['2026-11-01T07:30:00Z',true]]) {
   const routing=store.routing(Date.parse(date));assert.equal(routing.quietHoursActive,muted,date);assert.equal(routing.notifyPhone,!muted,date);
  }
  store.setQuietHours({...schedule,start:'09:00',end:'17:00'});
  assert.equal(store.routing(Date.parse('2026-10-02T15:00:00Z')).notifyPhone,false);
  store.setQuietHours({...schedule,enabled:false});
  assert.equal(store.routing(Date.parse('2026-10-02T03:00:00Z')).notifyPhone,true);
 } finally {store.close();}
});
test('quiet hours suppress every notification kind and queued retries without a morning burst',async()=>{
 const store=new HubStore();let clock=Date.parse('2026-10-02T02:59:59Z');const sent=[];
 try {
  store.setQuietHours(schedule);store.setPreferences({mode:'always',awayAfterSeconds:60});
  const {id}=store.pair(store.pairing());store.db.prepare('UPDATE devices SET subscription=? WHERE id=?').run('{}',id);
  const app=createApp({store,now:()=>clock,hubToken:'x'.repeat(64),vapid:{},push:{setVapidDetails(){},async sendNotification(_,body){sent.push(body);}}});
  for(const kind of ['alert','update','prompt','approval']){
   store.upsert({taskID:kind,kind,createdAt:clock/1000});store.enqueue({id:kind},clock);
  }
  clock+=1000;await app.locals.pump();assert.equal(sent.length,0);
  store.upsert({taskID:'overnight',kind:'prompt',createdAt:clock/1000});store.enqueue({id:'overnight'},clock);
  clock=Date.parse('2026-10-02T12:00:00Z');await app.locals.pump();assert.equal(sent.length,0);
  assert.equal(store.get('overnight').status,'pending');
  store.upsert({taskID:'morning',kind:'prompt',createdAt:clock/1000});store.enqueue({id:'morning'},clock);await app.locals.pump();assert.equal(sent.length,1);
 } finally {store.close();}
});
