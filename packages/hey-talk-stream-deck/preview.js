import {keys, artwork} from "./keys.js";
import {mkdir, writeFile} from "node:fs/promises";
import {execFileSync} from "node:child_process";
const dest = process.argv[2] ?? "dist/preview";
await mkdir(dest,{recursive:true});
const states={
 dictation:["Ready","Connecting","Listening","Recording","Processing","Paste wait","Done","Notice","Failed"],
 conversation:["Ready","Starting","Listening","Recording","Transcribing","Thinking","Speaking","Paused","Muted","Approval","Voice off"],
 stop:["Unavailable","Stop"], mute:["Unavailable","Live","Muted"], interrupt:["Unavailable","Ready"],
 transcript:["Unavailable","Open"], "new-chat":["Start"],auto:["Off","On"], submit:["Off","On"],send:["Unavailable","Send"],
 correction:["Off","On"],return:["Off","On"],speech:["Off","On"],play:["Ready","Loading","Playing","Paused","Failed"],
 "catch-up":["Ready","Loading","Playing","Paused","Failed"],pill:["Hidden","Shown"],history:["Open"],vocabulary:["Open"],permissions:["Open"]
};
const svgURI=svg=>"data:image/svg+xml;base64,"+Buffer.from(svg).toString("base64");
const items=(id,all=true)=>(all?[...states[id],"Offline","Update app"]:states[id]).map(state=>artwork(id,state,{level:4,progress:6}));
const groups={"Mini · first page":["dictation","conversation","stop","auto","catch-up"].map(id=>artwork(id,states[id][0]))};
groups["Action list icons"]=Object.entries(keys).map(([id,[name,,d]])=>'<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144"><rect width="144" height="144" rx="10" fill="#12161f"/><path transform="translate(48 31) scale(2)" d="'+d+'" fill="none" stroke="white" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/><text x="72" y="111" font-size="16" font-family="Helvetica, sans-serif" text-anchor="middle" fill="white">'+name+'</text></svg>');
for(const id of Object.keys(keys)) groups[keys[id][0]]=items(id);
groups["Auto mode & badges"]=[false,true].flatMap(auto=>[false,true].flatMap(c=>[false,true].map(s=>artwork("dictation","Recording",{auto:auto?"On":"Off",correction:c?"Off":"On",submit:s?"Off":"On",level:4}))));
groups["Live meter & progress"]=[...Array.from({length:6},(_,level)=>artwork("dictation","Recording",{level})),...Array.from({length:10},(_,progress)=>artwork("dictation","Processing",{progress}))];
const name="Hey Talk · Stream Deck";
const body='<style>*{box-sizing:border-box}section{padding:16px;border:1px solid var(--border,#434755);border-radius:var(--radius,12px)}label{display:block;font:600 13px system-ui;color:var(--muted-foreground,#8893a5);margin-bottom:6px}select{font:600 16px system-ui;max-width:100%;padding:6px 10px;border-radius:7px;background:var(--card,#222938);color:var(--foreground,#edf1f7);border:1px solid var(--border,#505a6b)}.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(86px,1fr));gap:8px;margin-top:14px}.grid img{width:100%;max-width:120px;display:block}small{font:11px system-ui;display:block;margin-top:4px}p{font:13px system-ui;color:var(--muted-foreground,#9199a8)}</style><section><label for="pick">HEY TALK / KEY PREVIEW</label><select id="pick">'+Object.keys(groups).map(n=>'<option>'+n+'</option>').join("")+'</select><div id="grid" class="grid"></div><p>Choose a key to see its states. The meter and progress views show individual frames.</p></section>';
const caption = (s,label,i)=>label==="Auto mode & badges" ? [(i&4)?"Auto":"Manual",(i&2)?"AI off":"",(i&1)?"Submit off":""].filter(Boolean).join(" · ") : label==="Live meter & progress" ? (i<6?"Level "+i:"Progress "+(i-6)+"/10") : s.match(/<title>([^<]*)<\/title>/)?.[1] ?? "";
const captions=Object.fromEntries(Object.entries(groups).map(([label,svgs])=>[label,svgs.map((s,i)=>caption(s,label,i))]));
const imgs=Object.fromEntries(Object.entries(groups).map(([label,svgs])=>[label,svgs.map(svgURI)]));
const page='<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>'+name+'</title>'+body+'<script>const groups='+JSON.stringify(imgs)+';const captions='+JSON.stringify(captions)+';function draw(){grid.replaceChildren(...groups[pick.value].map((src,i)=>{let tile=document.createElement("div"),cap=document.createElement("small");cap.textContent=captions[pick.value][i];tile.append(Object.assign(new Image(),{src,alt:cap.textContent}),cap);return tile}));}pick.onchange=draw;pick.value="Dictation";draw();</script>';
await writeFile(dest+"/gallery.html",page);
// Each sheet is rasterized from the exact SVG sent in setImage, including key typography.
for(const [slug, labels] of Object.entries({
 voice:["Dictation","Conversation","Auto mode & badges","Live meter & progress"],
 controls:Object.values(keys).map(k=>k[0]).filter(n=>!["Dictation","Conversation"].includes(n)),
 icons:["Action list icons"]
})){
  let y=68,content='';
  for(const label of labels){
    content+='<text x="24" y="'+y+'" fill="#d7dfed" font-size="20">'+label.replaceAll("&","&amp;")+'</text>';y+=12;
    groups[label].forEach((s,i)=>{content+='<image x="'+(24+i%8*112)+'" y="'+(y+Math.floor(i/8)*136)+'" width="104" height="104" href="'+svgURI(s)+'"/><text x="'+(76+i%8*112)+'" y="'+(y+Math.floor(i/8)*136+121)+'" text-anchor="middle" font-size="10" fill="#c1c9d8">'+(captions[label][i].split(" · ").slice(label==="Auto mode & badges"?0:1).join(" · ") || captions[label][i])+'</text>';});
    y+=Math.ceil(groups[label].length/8)*136+32;
  }
  const svg='<svg xmlns="http://www.w3.org/2000/svg" width="936" height="'+y+'"><rect width="100%" height="100%" fill="#0e121a"/><g font-family="Helvetica, sans-serif"><text x="24" y="28" fill="#a1afff" font-size="14">HEY TALK   /   STREAM DECK</text>'+content+'</g></svg>';
  execFileSync("rsvg-convert",["-o",dest+"/"+slug+".png"],{input:svg});
}
