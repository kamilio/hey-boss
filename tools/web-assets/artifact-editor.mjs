// A plain writing surface whose DOM stays bounded for large Markdown drafts.
import {EditorState,Compartment} from '@codemirror/state';
import {EditorView,keymap,drawSelection} from '@codemirror/view';
import {defaultKeymap,history,historyKeymap} from '@codemirror/commands';
window.HeyBossArtifactEditor=(parent,body,onChange)=>{
  const editable=new Compartment();
  const root=parent.attachShadow({mode:'open'});
  const view=new EditorView({parent:root,state:EditorState.create({doc:body,extensions:[
    history(),drawSelection(),keymap.of([...defaultKeymap,...historyKeymap]),EditorView.lineWrapping,
    EditorView.contentAttributes.of({'aria-label':'Markdown',spellcheck:'true'}),
    editable.of(EditorView.editable.of(true)),
    EditorView.updateListener.of(update=>{if(update.docChanged)onChange();}),
    EditorView.theme({
      '&':{background:'transparent',color:'var(--text)'},
      '.cm-scroller':{fontSize:'14px',fontFamily:'ui-monospace, SFMono-Regular, monospace',lineHeight:'1.85',overflow:'auto',height:'65vh',minHeight:'380px'},
      '.cm-content':{padding:'0',caretColor:'var(--accent)'},
      '.cm-line':{padding:'0'},
      '.cm-selectionBackground, &.cm-focused .cm-selectionBackground':{background:'var(--accent-soft)'},
      '&.cm-focused':{outline:'none'},
      '.cm-cursor':{borderLeftColor:'var(--text)'},
      '@media (max-width: 560px)':{'.cm-scroller':{fontSize:'16px'}}
    })
  ]})});
  return {
    content:()=>view.state.doc.toString(),
    focus:()=>view.focus(),
    replace:body=>view.dispatch({changes:{from:0,to:view.state.doc.length,insert:body}}),
    readOnly:value=>view.dispatch({effects:editable.reconfigure(EditorView.editable.of(!value))}),
    destroy:()=>view.destroy()
  };
};
