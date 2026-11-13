import test from "node:test";
import assert from "node:assert/strict";
import {once} from "node:events";
import {spawn} from "node:child_process";
import {mkdir, mkdtemp, rm} from "node:fs/promises";
import net from "node:net";
import path from "node:path";
import {WebSocketServer} from "ws";
import {prefix} from "./keys.js";

// Exercise the launched plugin with both real protocols, including external state changes.
test("keys register, follow external changes, route presses once, and go offline", {timeout:10000}, async t => {
  const home = await mkdtemp(path.resolve(".deck-"));
  t.after(()=>rm(home, {recursive:true, force:true}));
  // Relative bind keeps the test socket within Darwin's 104-byte path limit.
  const directory = path.join(home, "Library/Application Support/hey-talk");
  await mkdir(directory, {recursive:true});
  const calls = [];
  let stream;
  const server = net.createServer(s => s.once("data", data => {
    const {command, action} = JSON.parse(data);
    if (command === "watch_deck") { stream = s; s.write('{"ok":true,"conversation":"Ready"}\n'); return; }
    assert.equal(command, "deck");
    if (action) calls.push(action);
    s.end(JSON.stringify({ok:true, conversation:"Ready"})+"\n");
  }));
  const old = process.cwd();
  process.chdir(home);
  server.listen("Library/Application Support/hey-talk/control.sock");
  await once(server,"listening");
  process.chdir(old);
  t.after(()=>server.close());
  const host = new WebSocketServer({port:0, host:"127.0.0.1"});
  await once(host,"listening");
  t.after(()=>host.close());
  const connected = once(host,"connection");
  const child = spawn(process.execPath, [path.resolve("plugin.js"), "-port", String(host.address().port),
    "-pluginUUID","test","-registerEvent","registerPlugin"], {env:{...process.env, HOME:"."}, cwd:home, stdio:"inherit"});
  t.after(()=>child.kill());
  const [client] = await connected;
  t.after(()=>client.terminate());
  const events = [];
  client.on("message", data=>events.push(JSON.parse(data)));
  const until = async f => {
    for(let i=0;i<100;i++){ if(f()) return; await new Promise(r=>setTimeout(r,25)); }
    assert.fail("missing expected event: "+JSON.stringify(events.map(e=>e.payload?.title ?? e.event)));
  };
  await until(()=>events.some(e=>e.event==="registerPlugin" && e.uuid==="test"));
  const send = (event,context)=>client.send(JSON.stringify({event,context,action:prefix+".conversation"}));
  const matches = (e,c,s)=>e.context===c && e.payload?.image && Buffer.from(e.payload.image.split(",")[1],"base64").toString().includes(" · "+s+"</title>");
  const title = (c,s)=>events.some(e=>matches(e,c,s));
  send("willAppear","a"); send("willAppear","b");
  await until(()=>title("a","Ready") && title("b","Ready"));
  send("willDisappear","a");
  send("keyDown","b"); send("keyUp","b");
  await until(()=>calls.length===1);
  assert.deepEqual(calls,["conversation"]);
  stream.write('{"ok":true,"conversation":"Listening"}\n');
  await until(()=>title("b","Listening"));
  assert.equal(title("a","Listening"),false);
  stream.end();
  await new Promise(r=>server.close(r));
  await until(()=>events.filter(e=>matches(e,"b","Offline")).length===2);
});
