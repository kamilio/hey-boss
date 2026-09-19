import {test} from 'node:test';
import assert from 'node:assert/strict';
import {activityGroups,activityDateTime,activityTime} from '../src/activity.js';
test('Activity uses local calendar days and safely handles legacy or invalid dates',()=>{
 const now=new Date(2026,8,16,0,5),seconds=date=>date.getTime()/1000;
 const rows=[{taskID:'unknown'},{taskID:'today',completedAt:seconds(now)},{taskID:'yesterday',completedAt:seconds(new Date(2026,8,15,23,59))},{taskID:'invalid',completedAt:'broken'}];
 const groups=activityGroups(rows,now);assert.deepEqual(groups.map(g=>g.label),['Today','Yesterday','Earlier']);assert.deepEqual(groups[2].tasks.map(t=>t.taskID),['unknown','invalid']);
 assert.equal(activityDateTime(rows[1]),now.toISOString());assert.equal(activityDateTime(rows[3]),undefined);assert.equal(activityTime(rows[3]),'');
});
