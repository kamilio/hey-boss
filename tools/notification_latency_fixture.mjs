// Disposable relay: hold responses long enough to detect UI/store queue blocking.
import http from 'node:http';
const server = http.createServer(async (req, res) => {
  let raw = ''; for await (const chunk of req) raw += chunk;
  const body = raw ? JSON.parse(raw) : {};
  const send = (code, value) => { res.writeHead(code, {'content-type':'application/json'}); res.end(JSON.stringify(value)); };
  await new Promise(resolve => setTimeout(resolve, 700));
  if (req.url.endsWith('/clear')) {
    if (body.taskIDs.includes('dismiss-fail')) return send(503, {error:'Synthetic relay unavailable'});
    return send(200, {tasks:body.tasks.map(task => ({taskID:task.taskID,status:task.kind === 'approval' || task.kind === 'prompt' || task.commentsEnabled ? 'cancelled' : 'ok'}))});
  }
  const match = req.url.match(/\/tasks\/([^/]+)\/resolve$/);
  if (!match) return send(404, {error:'Unknown fixture path'});
  const id = match[1];
  if (id.endsWith('-fail')) return send(503, {error:'Synthetic relay unavailable'});
  send(id === 'phone-won' ? 409 : 200, {task:{taskID:id,status:'ok',result:id === 'phone-won' ? 'Phone answer' : body.result}});
});
server.listen(0, '127.0.0.1', () => console.log(`http://127.0.0.1:${server.address().port}`));
