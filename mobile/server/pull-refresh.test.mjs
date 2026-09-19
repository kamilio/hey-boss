import {test} from 'node:test';
import assert from 'node:assert/strict';
import {pullGesture,movePull,readyPull} from '../src/pull-refresh.js';
test('pull refresh requires a deliberate downward drag from the top',()=>{
 assert.equal(pullGesture({scrollTop:1,x:0,y:0}),null);
 assert.equal(pullGesture({blocked:true,x:0,y:0}),null);
 for(const [x,y] of [[100,40],[0,-200]]){
  const gesture=pullGesture({x:0,y:0});movePull(gesture,x,y);movePull(gesture,0,300);
  assert.equal(readyPull(gesture),false);
 }
 const gesture=pullGesture({x:0,y:0});movePull(gesture,2,100);assert.equal(readyPull(gesture),false);
 movePull(gesture,2,150);assert.equal(readyPull(gesture),true);
 assert.equal(movePull(gesture,2,10000),88);
 movePull(gesture,2,20);assert.equal(readyPull(gesture),false);
});
