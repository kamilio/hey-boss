import {miniProfile} from "./profile.js";
import {mkdir, writeFile, copyFile, readFile} from "node:fs/promises";
import {build} from "esbuild";
import {keys, hiddenKeys, prefix, artwork} from "./keys.js";
const dir = prefix + ".sdPlugin";
await mkdir(dir + "/icons", {recursive:true});
await mkdir("dist", {recursive:true});
await copyFile("plugin.png", dir+"/plugin.png"); await copyFile("plugin@2x.png", dir+"/plugin@2x.png");
const version = JSON.parse(await readFile("package.json")).version+".0";
await miniProfile(dir,version);
const actions = [];
for (const [id, [Name, Tooltip, path]] of Object.entries(keys)) {
  await writeFile(dir + "/icons/" + id + ".svg", '<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24"><path d="'+path+'" fill="none" stroke="white" stroke-width="1.8" stroke-linejoin="round" stroke-linecap="round"/></svg>');
  await writeFile(dir + "/icons/key-" + id + ".svg", artwork(id, "Ready"));
  actions.push({UUID: prefix+"."+id, Name, Tooltip, Icon:"icons/"+id, Controllers:["Keypad"],
    VisibleInActionsList:!hiddenKeys.has(id), SupportedInMultiActions:false, States:[{Image:"icons/key-"+id, ShowTitle:false}]});
}
await writeFile(dir+"/manifest.json", JSON.stringify({UUID:prefix, Name:"Hey Talk", Category:"Hey Talk",
  CategoryIcon:"icons/auto", Author:"Hey Talk", Description:"Live dictation, voice conversation and playback controls for Hey Talk.",
  Profiles:[{DeviceType:1,Name:"Hey Talk Mini"}], Version:version, Icon:"plugin", CodePath:"plugin.js", SDKVersion:2, Nodejs:{Version:"20"},
  Software:{MinimumVersion:"6.6"}, OS:[{Platform:"mac",MinimumVersion:"15"}], Actions:actions}, null, 2));
await build({entryPoints:["plugin.js"], outfile:dir+"/plugin.js", bundle:true, platform:"node", format:"cjs", target:"node20"});
