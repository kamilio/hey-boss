import {createApp} from './index.mjs';import {HubStore} from './store.mjs';import fs from 'node:fs';
const store=new HubStore();const app=createApp({store,hubToken:'native-audit-local-token-000000000000',origin:'http://127.0.0.1',secure:false});
app.post('/test/phone/:id',(req,res)=>res.json({task:store.resolve(req.params.id,req.body.result,'phone')}));
app.post('/test/open/:id',(req,res)=>res.json({task:store.open(req.params.id)}));
app.get('/test/document/:id',(req,res)=>res.json({task:store.get(req.params.id)}));
const counts={};app.get('/test/counts',(req,res)=>res.json({counts}));
const server=app.listen(0,'127.0.0.1',()=>fs.writeFileSync(process.argv[2],String(server.address().port)));
server.on('request',req=>{const key=req.method+' '+req.url;counts[key]=(counts[key]??0)+1;});
process.on('SIGTERM',()=>{app.locals.close();server.close(()=>{store.close();process.exit(0);});});
