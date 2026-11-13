import WebSocket from "ws";
import net from "node:net";
import {homedir} from "node:os";
import {join} from "node:path";
import {createInterface} from "node:readline";
import {execFile} from "node:child_process";
import {promisify} from "node:util";
import {setTimeout as delay} from "node:timers/promises";
import {keys, prefix, artwork} from "./keys.js";

const args = process.argv.slice(2);
const arg = name => args[args.indexOf(name)+1];
const host = new WebSocket("ws://127.0.0.1:"+arg("-port"));
const path = join(homedir(), "Library/Application Support/hey-talk/control.sock");
const visible = new Map();
let frame = {}, missing = "Offline", watcher, retry;
const send = event => host.readyState === WebSocket.OPEN && host.send(JSON.stringify(event));

function draw(context, key) {
  const state = frame[key.id] ?? missing;
  const level = ["Recording","Listening"].includes(state) ? frame.level ?? 0 : 0;
  const stamp = [state,level,...(key.id==="dictation" ? [frame.auto,frame.submit,frame.correction,frame.progress] : [])].join("|");
  if (key.stamp === stamp) return;
  const image = "data:image/svg+xml;base64,"+Buffer.from(artwork(key.id,state,frame)).toString("base64");
  send({event:"setImage", context, payload:{image}});
  key.stamp = stamp;
}
function redraw() { for (const [context,key] of visible) draw(context,key); }
function watch() {
  if (watcher || !visible.size) return;
  const socket = watcher = net.createConnection(path);
  socket.on("connect", ()=>socket.write('{"command":"watch_deck"}\n'));
  createInterface({input:socket}).on("error", ()=>socket.destroy()).on("line", line=>{
    if (watcher !== socket) return;
    try {
      const data = JSON.parse(line);
      frame = data.ok ? data : {};
      missing = data.ok ? "Offline" : "Update app";
      redraw();
    } catch { socket.destroy(); }
  });
  socket.on("error", ()=>{});
  socket.on("close", ()=>{
    if (watcher !== socket) return;
    watcher = undefined; frame = {}; redraw();
    if (visible.size) retry = setTimeout(watch,1000);
  });
}
function press(id) {
  return new Promise((resolve,reject)=>{
    const socket = net.createConnection(path);
    socket.setTimeout(15000, ()=>socket.destroy(new Error("No reply")));
    socket.once("error",reject);
    socket.on("connect", ()=>socket.write(JSON.stringify({command:"deck",action:id})+"\n"));
    createInterface({input:socket}).on("error",reject).once("line", line=>{
      try { JSON.parse(line).ok ? resolve() : reject(new Error("Action unavailable")); }
      catch (e) { reject(e); }
      socket.destroy();
    });
    socket.on("close", ()=>reject(new Error("Disconnected")));
  });
}
async function keyDown(id) {
  try { await press(id); }
  catch (e) {
    if (!["ENOENT","ECONNREFUSED"].includes(e.code)) throw e;
    // Launch without taking focus from the dictation destination. Only retry a
    // failed connection: a lost reply must never execute the same press twice.
    await promisify(execFile)("/usr/bin/open", ["-gj","-b","local.hey-talk","--args","--serve"]);
    for(let i=0;i<30;i++) {
      await delay(150);
      try { await press(id); return; }
      catch (e) { if (!["ENOENT","ECONNREFUSED"].includes(e.code)) throw e; }
    }
    throw new Error("Hey Talk did not start");
  }
}
host.on("open", ()=>send({event:arg("-registerEvent"),uuid:arg("-pluginUUID")}));
host.on("message", raw=>{
  const {event,action,context} = JSON.parse(raw);
  if (!action?.startsWith(prefix+".")) return;
  const id = action.slice(prefix.length+1);
  if (!Object.hasOwn(keys,id)) return;
  const key = visible.get(context);
  const invoke = action => keyDown(action).catch(()=>send({event:"showAlert",context}));
  if (event === "willAppear") {
    clearTimeout(visible.get(context)?.hold);
    const key = {id}; visible.set(context,key); draw(context,key); clearTimeout(retry); watch();
  } else if (event === "willDisappear") {
    clearTimeout(key?.hold);
    visible.delete(context);
    if (!visible.size) { clearTimeout(retry); watcher?.destroy(); watcher=undefined; frame={}; missing="Offline"; }
  } else if (id === "dictation" && key) {
    if (event === "keyDown" && key.hold === undefined) {
      key.hold = setTimeout(()=>{
        key.hold = null; // Release after a hold must not toggle dictation.
        if (!frame.conversation || ["Ready","Voice off"].includes(frame.conversation)) invoke("new-chat");
      }, 500);
    } else if (event === "keyUp") {
      if (key.hold) { clearTimeout(key.hold); invoke("dictation"); }
      delete key.hold;
    }
  } else if (id !== "dictation" && event === "keyDown") {
    invoke(id);
  }
});
host.on("error", ()=>process.exit(1));
host.on("close", ()=>process.exit(0));
