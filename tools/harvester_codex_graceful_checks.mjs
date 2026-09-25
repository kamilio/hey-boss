// Real Codex TUI against a local synthetic Responses server; no account or API key.
import assert from 'node:assert/strict';
import http from 'node:http';
import {mkdtemp,mkdir,writeFile,rm,realpath} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
import {TerminalPilot} from '../worker-tui/node_modules/terminal-pilot/dist/index.js';
const root=await realpath(await mkdtemp(join(tmpdir(),'harvester-codex-exit-')));
const home=join(root,'codex'),cwd=join(root,'work');await mkdir(home);await mkdir(cwd);
let served=false;
const server=http.createServer((req,res)=>{
 req.resume();
 if(!req.url.endsWith('/responses')) {res.writeHead(404);res.end();return;}
 const item={id:'msg_fixture',type:'message',role:'assistant',status:'completed',content:[{type:'output_text',text:'HARVESTER_FIXTURE_DONE',annotations:[]}]};
 res.writeHead(200,{'content-type':'text/event-stream','cache-control':'no-cache'});
 const emit=o=>res.write('data: '+JSON.stringify(o)+'\n\n');
 emit({type:'response.created',response:{id:'resp_fixture',status:'in_progress',output:[]}});
 emit({type:'response.output_item.added',output_index:0,item:{...item,status:'in_progress',content:[]}});
 emit({type:'response.output_text.delta',item_id:item.id,output_index:0,content_index:0,delta:'HARVESTER_FIXTURE_DONE'});
 emit({type:'response.output_item.done',output_index:0,item});
 emit({type:'response.completed',response:{id:'resp_fixture',status:'completed',output:[item],usage:{input_tokens:1,output_tokens:1,total_tokens:2}}});
 res.end();served=true;
});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
await writeFile(join(home,'config.toml'),`model = "fixture"\nmodel_provider = "fixture"\n[model_providers.fixture]\nname = "Fixture"\nbase_url = "http://127.0.0.1:${server.address().port}/v1"\nwire_api = "responses"\nrequires_openai_auth = false\n[projects.${JSON.stringify(cwd)}]\ntrust_level = "trusted"\n`);
const pilot=await TerminalPilot.launch();
try {
 const s=await pilot.newSession({command:resolve(process.argv[2]),args:['--no-alt-screen','--disable','plugins'],cwd,cols:100,rows:30,env:{...process.env,CODEX_HOME:home,TERM:'xterm-256color'}});
 await s.waitFor(/model:\s+fixture/, {timeout:30000,scope:'history'});
 await delay(500);
 if(s.exitCode!==null)throw new Error((await s.history({last:20})).join('\n'));
 await s.type('Reply with the fixture response.');await delay(500);await s.press('enter');
 for(let i=0;i<40&&!served&&s.exitCode===null;i++)await delay(500);
 assert(served,(await s.history({last:25})).join('\n'));
 await s.waitFor('HARVESTER_FIXTURE_DONE',{timeout:10000,scope:'history'});await delay(1000);
 await s.send('\x04');
 const code=await s.waitForExit({timeout:10000});
 const transcript=(await s.history()).join('\n');
 assert.equal(code,0,transcript);
 assert.match(transcript,/codex resume [0-9a-f-]{36}/,transcript);
 console.log(JSON.stringify({normalCompletion:true,completed:3,expected:3,checks:['Synthetic turn completed in real Codex TUI','Ctrl-D exited normally without signals','Codex printed its session resume command']}));
} catch(e) {console.error(e); throw e;} finally {await pilot.close();server.closeAllConnections();await new Promise(r=>server.close(r));await rm(root,{recursive:true,force:true,maxRetries:5,retryDelay:500}).catch(e=>console.error('Fixture cleanup:',e.message));}
