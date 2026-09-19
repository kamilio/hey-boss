export const PULL_THRESHOLD=64;
export function pullGesture({scrollTop=0,blocked=false,x,y}){
 if(scrollTop>0||blocked)return null;
 return {x,y,direction:null,distance:0};
}
export function movePull(gesture,x,y){
 const dx=x-gesture.x,dy=y-gesture.y;
 if(!gesture.direction&&Math.max(Math.abs(dx),Math.abs(dy))>8)
  gesture.direction=dy>0&&dy>Math.abs(dx)*1.2?'down':'cancel';
 gesture.distance=gesture.direction==='down'?Math.min(88,Math.max(0,dy)*0.45):0;
 return gesture.distance;
}
export function readyPull(gesture){return gesture?.direction==='down'&&gesture.distance>=PULL_THRESHOLD;}
