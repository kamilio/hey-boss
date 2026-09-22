// Delayed real mobile hub for the native Close all performance audit.
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';
import express from '../mobile/node_modules/express/index.js';
const store=new HubStore();
store.upsert({taskID:'bulk-0',kind:'approval',title:'Review 0',question:'Ready for review',description:'Checks passed',options:['Approve','Reject']});
store.resolve('bulk-0','Approve','phone');
let clearCalls=0,resolveCalls=0,publishCalls=0,failNext=false;
const app=express();
app.get('/metrics',(_req,res)=>res.json({clearCalls,resolveCalls,publishCalls}));
app.post('/fail',(_req,res)=>{failNext=true;res.json({ok:true});});
app.use((req,res,next)=>{
 if(req.path==='/api/bridge/tasks/clear'){clearCalls++;setTimeout(()=>{if(failNext){failNext=false;res.status(503).json({error:'Synthetic relay outage'});}else next();},150);}
 else if(req.path.endsWith('/resolve')){resolveCalls++;setTimeout(next,150);}
 else {if(req.method==='PUT')publishCalls++;next();}
});
const hub=createApp({store,hubToken:'x'.repeat(32),origin:'http://127.0.0.1',secure:false});
app.use(hub);
const server=app.listen(0,'127.0.0.1',()=>console.log('http://127.0.0.1:'+server.address().port));
process.on('SIGTERM',()=>{hub.locals.close();server.close();store.close();});
