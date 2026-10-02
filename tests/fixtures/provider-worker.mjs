#!/usr/bin/env node
import {readFileSync,writeFileSync,appendFileSync,existsSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {createInterface} from 'node:readline';
const provider=process.env.HEY_BOSS_FIXTURE_PROVIDER;
appendFileSync('launches.jsonl',JSON.stringify(process.argv.slice(2))+'\n');
const session='12345678-1234-1234-1234-123456789abc';
const file=process.cwd()+'/session.jsonl';
if(provider==='pi')writeFileSync(file,JSON.stringify({type:'session',id:session})+'\n');
const send=v=>process.stdout.write(JSON.stringify(v)+'\n');
let streaming=false, turns=0, timer;
const report=()=>JSON.stringify({status:'completed',summary:'Provider worker verified'});
function complete(output){
  if(output===report()){
    const result=spawnSync(process.env.HEY_BOSS_TEST_CLI,['issue','close',process.env.HEY_BOSS_ISSUE_NUMBER],{encoding:'utf8'});
    if(result.status)throw Error(result.stderr);
  }
  streaming=false;
  if(provider==='claude')send({type:'result',session_id:session,subtype:'success',is_error:false,result:output});
  else {send({type:'message_end',message:{role:'assistant',content:[{type:'text',text:output}],stopReason:'stop'}});send({type:'agent_settled'});}
}
function prompt(text){
  turns++;streaming=true;appendFileSync('inputs.jsonl',JSON.stringify(text)+'\n');
  if(provider==='claude')send({type:'system',subtype:'init',session_id:session});
  if(turns===1){
    const result=spawnSync(process.env.HEY_BOSS_TEST_CLI,['issue','claim',process.env.HEY_BOSS_ISSUE_NUMBER],{encoding:'utf8'});
    if(result.status)throw Error(result.stderr);
    writeFileSync('claimed',provider);
  }
  if(provider==='claude')send({type:'stream_event',session_id:session,event:{delta:{type:'text_delta',text:'Working'}}});
  else send({type:'message_update',assistantMessageEvent:{type:'text_delta',delta:'Working'}});
  if(text.includes('goal fixture')&&turns===1){setTimeout(()=>complete('More work remains'),100);return;}
  if(text.includes('Continue the saved goal')){complete(report());return;}
  if(text.includes('STEER')){complete(report());return;}
  timer=setInterval(()=>{if(existsSync('finish')){clearInterval(timer);complete(report());}},50);
}
createInterface({input:process.stdin}).on('line',line=>{
  const v=JSON.parse(line);appendFileSync('protocol.jsonl',line+'\n');
  if(provider==='claude'){
    if(v.type==='control_request')send({type:'control_response',response:{subtype:'success',request_id:v.request_id,response:{}}});
    if(v.type==='user')prompt(v.message.content);
  } else {
    const reply=data=>send({type:'response',id:v.id,command:v.type,success:true,data});
    if(v.type==='get_state')reply({sessionId:session,sessionFile:file,isStreaming:streaming});
    else if(v.type==='prompt'){reply({});prompt(v.message);}
    else if(v.type==='steer'){reply({});appendFileSync('inputs.jsonl',JSON.stringify(v.message)+'\n');clearInterval(timer);complete(report());}
    else reply({});
  }
});
