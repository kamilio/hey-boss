// 24-point line icons; the same shapes identify a key in the action list and on the deck.
export const prefix = "local.heytalk.deck";
export const hiddenKeys = new Set(["submit","correction","return","pill","history","vocabulary","permissions"]);
export const keys = {
  dictation: ["Dictation", "Start or finish dictation, including during a conversation.", "M7 9v6 M12 5v14 M17 7.5v9"],
  conversation: ["Conversation", "Start voice chat, then use the current conversation action.", "M3 4h13v10H8l-5 4z M17 9h4v12l-4-3h-5v-2"],
  stop: ["Stop", "Stop conversation or playback, or cancel dictation.", "M5 5h14v14H5z"],
  mute: ["Chat Mic", "Pause or resume listening in the current chat.", "M4 4l16 16 M9 5a3 3 0 0 1 6 0v5 M6 11a6 6 0 0 0 10 5 M12 18v4 M8 22h8"],
  interrupt: ["Next Turn", "Interrupt the reply and listen for the next message.", "M4 11h12 M11 5l6 6-6 6 M20 5v12"],
  transcript: ["Transcript", "Show the current conversation transcript.", "M4 3h16v18H4z M8 8h8 M8 12h8 M8 16h5"],
  "new-chat": ["New Chat", "Start listening with an empty chat.", "M3 4h18v13H9l-6 5z M8 10h8 M12 6v8"],
  auto: ["Auto Dictate", "Toggle hands-free dictation.", "M3 10v4 M7 7v10 M12 3v18 M17 7v10 M21 10v4"],
  send: ["Send Now", "Finish the active automatic dictation like pressing Enter.", "M3 4l18 8-18 8 3-8z M6 12h13"],
  submit: ["Auto Submit", "Toggle Enter after paste for the focused app or website.", "M20 5v9H4 M10 8l-6 6 6 6"],
  correction: ["AI Correction", "Toggle transcript correction.", "M12 5c-1-5-8-3-7 2-4 1-4 6-2 8-2 5 5 8 9 4 4 4 11 1 9-4 2-2 2-7-2-8 1-5-6-7-7-2z M12 5v14 M5 7l3 2 M19 7l-3 2 M3 15l4-1 M21 15l-4-1"],
  return: ["Return to Field", "Toggle returning focus to the original input before pasting.", "M4 7h16v13H4z M8 3v9 M5 9l3 3 3-3 M8 16h8"],
  speech: ["Read Aloud", "Enable or disable the paragraph Listen control.", "M3 9h4l5-5v16l-5-5H3z M16 8q4 4 0 8 M19 4q8 8 0 16"],
  play: ["Playback", "Read selected or hovered text, or pause and resume playback.", "M4 4l10 8-10 8z M17 6v12 M21 6v12"],
  "catch-up": ["Catch Up", "Hear a recap of the focused conversation or note, then talk.", "M3 11a9 9 0 1 1 2 7 M3 5v6h6 M11 7v6l4 2"],
  pill: ["Pill", "Show or hide the floating control.", "M7 6h10a6 6 0 0 1 0 12H7A6 6 0 0 1 7 6z M7 11v2 M11 9v6 M15 11v2"],
  history: ["History", "Open dictation history.", "M3 11a9 9 0 1 1 2 7 M3 5v6h6 M12 6v6l4 3"],
  vocabulary: ["Vocabulary", "Open the vocabulary editor.", "M12 6Q7 3 2 5v14q5-2 10 1 5-3 10-1V5q-5-2-10 1z M12 6v14"],
  permissions: ["Permissions", "Open paste permission setup.", "M12 2l8 3v6q0 6-8 11-8-5-8-11V5z M8 11l3 3 5-6"]
};

const symbols = {
  mic: "M9 5a3 3 0 0 1 6 0v6a3 3 0 0 1-6 0z M5 10v1a7 7 0 0 0 14 0v-1 M12 18v4 M8 22h8",
  pause: "M8 5v14 M16 5v14", resume: "M6 4l13 8-13 8z",
  busy: "M5 3h14 M5 21h14 M7 3c0 5 1 6 5 9-4 3-5 4-5 9 M17 3c0 5-1 6-5 9 4 3 5 4 5 9",
  hand: "M6 12V6a1.5 1.5 0 0 1 3 0v5-7a1.5 1.5 0 0 1 3 0v7-8a1.5 1.5 0 0 1 3 0v8-6a1.5 1.5 0 0 1 3 0v7l1-2q3-3 3 0l-3 9q-2 4-7 3-4 0-6-3l-4-6q0-3 2-2z",
  check: "M3 12l6 6L21 5", cross: "M5 5l14 14 M19 5L5 19",
  notice: "M12 3L2 21h20z M12 9v5 M12 17v.1", wait: "M6 5h12v16H6z M9 3h6v4H9z M9 13h6",
};
// Keep the geometry and dark appearance colors in sync with IdentityView and TalkDesign.
export function artwork(id, state, frame = {}) {
  const offline = ["Offline","Update app"].includes(state);
  const dict = id==="dictation" && !offline, chat = id==="conversation";
  const automatic = dict && frame.auto==="On";
  const active = ["Listening","Recording"].includes(state);
  const color = offline || ["Unavailable","Muted","Hidden","Off"].includes(state) ?
    (state==="Off" && ["correction","submit"].includes(id) ? "#ff6961" : "#9aa3b3") :
    state==="Approval" || state==="Notice" ? "#ffb340" : state==="Failed" ? "#f79791" : state==="Done" ? "#8bd4a7" :
    automatic || id==="auto" && state==="On" ? "#d4b0ff" : active || state==="Live" ? "#80cfca" : "#a1afff";
  let path = keys[id][2];
  if(chat) path = ({Approval:symbols.hand,Listening:symbols.mic,Recording:symbols.mic,Muted:keys.mute[2],
    "Voice off":keys.mute[2],Paused:symbols.pause,Speaking:keys.speech[2],Starting:symbols.busy,Thinking:symbols.busy,Transcribing:symbols.busy})[state] ?? path;
  if(id==="mute" && state==="Live") path=symbols.pause;
  if(id==="mute" && state==="Muted") path=symbols.resume;
  if(dict) path=({Done:symbols.check,Failed:symbols.cross,Notice:symbols.notice,"Paste wait":symbols.wait})[state] ?? path;
  const glyph=(p,x,y,k,col,w=1.9)=>'<path transform="translate('+x+' '+y+') scale('+k+')" d="'+p+'" fill="none" stroke="'+col+'" stroke-width="'+w+'" stroke-linecap="round" stroke-linejoin="round"/>';
  if (["Connecting","Starting","Loading"].includes(state)) path="M12 2a10 10 0 1 1-9.5 7";
  if (state==="Thinking") path="M3 12h1 M11 12h1 M19 12h1";
  if (["play","catch-up"].includes(id)) {
    if(state==="Paused") path=symbols.resume;
    if(state==="Playing") path=symbols.pause;
    if(state==="Failed") path=symbols.cross;
  }
  const glassColor = state === "Unavailable" ? "#383e49" : color;
  const badge = (p,x,y,col)=>glyph(p,x,y,.7,col,2.6);
  const badges = dict && state!=="Paste wait" ? (frame.correction==="Off"?badge(keys.correction[2],106,46,"#ff6961"):"")+
      (frame.submit==="Off"?badge(keys.submit[2],106,79,"#ff6961"):""):"";
  const meter = state==="Recording" && (dict || chat) ? Array.from({length:11},(_,i)=>{
    const h=3+(frame.level??0)*[1,2,1.3,2.6,1.4,2,3,.8,1.6,2,1][i];
    return '<rect x="'+(66+3.55*i)+'" y="'+(72-h/2)+'" width="2.5" height="'+h+'" rx="1.25" fill="'+color+'"/>';
  }).join(""):"";
  if(dict && state==="Listening") path=symbols.mic;
  const capsule = 'x="9" y="39" width="126" height="66" rx="33"';
  const rim = state==="Processing" ? '<rect '+capsule+' fill="none" stroke="'+color+'" stroke-width="4" stroke-dasharray="'+(12+32*(frame.progress??0))+' 400"/>' : "";
  let indicator="";
  if (offline) indicator=badge(state==="Offline"?"M3 3l18 18 M5 11l5-5 M13 18l5-5":"M12 19V3 M5 10l7-7 7 7 M3 18v4h18v-4",104,83,"#ffb340");
  else if(state==="Hidden" || state==="Off") indicator=badge("M6 17L18 5",104,82,color);
  else if(state==="On" || state==="Shown") indicator=badge(symbols.check,104,82,color);
  return '<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144"><title>'+keys[id][0]+' · '+state+'</title><defs><linearGradient id="glass" x2="0" y2="1"><stop stop-color="#32394a"/><stop offset="1" stop-color="#171d28"/></linearGradient><linearGradient id="edge" x2="1" y2="1"><stop stop-color="#8c94a7"/><stop offset=".48" stop-color="'+glassColor+'"/><stop offset="1" stop-color="#535b6b"/></linearGradient></defs><rect width="144" height="144" rx="10" fill="#11151d"/><g opacity="'+(state==="Unavailable"?.38:1)+'"><rect '+capsule+' fill="url(#glass)" stroke="url(#edge)" stroke-width="'+(automatic?3:1.5)+'"/>'+rim+glyph(path,meter?18:47,47,2.05,color)+meter+badges+indicator+'</g></svg>';
}
