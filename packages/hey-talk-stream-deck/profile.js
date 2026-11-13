import {mkdir, writeFile, rm} from "node:fs/promises";
import {execFileSync} from "node:child_process";
import {resolve} from "node:path";
import {keys,prefix} from "./keys.js";

// Stream Deck's editable six-key profile. Page controls are native Elgato actions.
const pages = [
  ["dictation","conversation","stop","auto","catch-up","next"],
  ["mute","interrupt","transcript","previous","new-chat","next"],
  ["submit","correction","return","previous","send","next"],
  ["speech","play","pill","previous","history","next"],
  ["vocabulary","permissions",null,"previous"]
];
const guid = n => "cc888980-91af-4751-978e-"+String(n).padStart(12,"0");
export async function miniProfile(dir,version) {
  const root=guid(0).toUpperCase()+".sdProfile", ids=pages.map((_,i)=>guid(i+1));
  const base="dist/"+root;
  for(const [i,page] of pages.entries()){
    const path=base+"/Profiles/"+ids[i].toUpperCase();
    await mkdir(path,{recursive:true});
    const Actions={};
    page.forEach((id,j)=>{
      if(!id) return;
      const nav=id==="next" || id==="previous";
      const plugin=nav ? "com.elgato.streamdeck.page" : prefix;
      Actions[(j%3)+","+Math.floor(j/3)]={ActionID:guid(100+i*6+j), UUID:plugin+"."+id,
        LinkedTitle:true, Name:nav ? id+" Page" : keys[id][0], Settings:{},State:0,States:[{}],
        Plugin:{Name:nav?"Pages":"Hey Talk",UUID:plugin,Version:nav?"1.0":version}};
    });
    await writeFile(path+"/manifest.json",JSON.stringify({Controllers:[{Type:"Keypad",Actions}],Name:"",Icon:""}));
  }
  await writeFile(base+"/manifest.json",JSON.stringify({Version:"3.0",Name:"Hey Talk",Device:{Model:"20GAI9901",UUID:""},
    Pages:{Current:ids[0],Default:ids[0],Pages:ids}}));
  const output=resolve(dir,"Hey Talk Mini.streamDeckProfile");
  await rm(output,{force:true});
  execFileSync("zip",["-q","-r","-X",output,root],{cwd:"dist"});
  await rm(base,{recursive:true});
}
