import {useState,useEffect,useRef,useCallback} from 'react';
import {pullGesture,movePull,readyPull,PULL_THRESHOLD} from './pull-refresh';

export default function usePullRefresh(ref,enabled,refresh){
 const [distance,setDistance]=useState(0),[loading,setLoading]=useState(false);
 const running=useRef(false);
 const run=useCallback(async()=>{
  if(running.current)return;
  running.current=true;setLoading(true);setDistance(PULL_THRESHOLD);
  try{await refresh();}finally{running.current=false;setLoading(false);setDistance(0);}
 },[refresh]);
 useEffect(()=>{
  const element=ref.current;if(!element||!enabled){setDistance(0);return;}
  let gesture=null,id=null;
  const cancel=()=>{gesture=null;id=null;if(!running.current)setDistance(0);};
  const start=event=>{
   cancel();if(running.current||event.touches.length!==1)return;
   const touch=event.touches[0];id=touch.identifier;
   gesture=pullGesture({x:touch.clientX,y:touch.clientY,scrollTop:Math.max(window.scrollY,document.scrollingElement?.scrollTop??0),blocked:!!event.target.closest('button,a,input,textarea,select,[contenteditable],.table-scroll,pre')});
  };
  const move=event=>{
   if(!gesture)return;if(event.touches.length!==1){cancel();return;}
   const touch=[...event.touches].find(t=>t.identifier===id);if(!touch){cancel();return;}
   if(window.scrollY>0){cancel();return;}
   const value=movePull(gesture,touch.clientX,touch.clientY);
   if(gesture.direction==='down'){if(!event.cancelable){cancel();return;}event.preventDefault();setDistance(value);}
  };
  const end=()=>{const ready=readyPull(gesture);cancel();if(ready)run();};
  element.addEventListener('touchstart',start,{passive:true});
  element.addEventListener('touchmove',move,{passive:false});
  element.addEventListener('touchend',end);
  element.addEventListener('touchcancel',cancel);
  return()=>{element.removeEventListener('touchstart',start);element.removeEventListener('touchmove',move);element.removeEventListener('touchend',end);element.removeEventListener('touchcancel',cancel);};
 },[ref,enabled,run]);
 return {distance,loading,ready:distance>=PULL_THRESHOLD,run};
}
