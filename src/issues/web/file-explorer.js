// A folder browser with one active document. Adapters own their revision and save semantics.
window.HeyBossFiles = (() => {
  let editorScript;
  function loadEditor() {
    if (window.HeyBossArtifactEditor) return Promise.resolve();
    return editorScript ||= new Promise((resolve, reject) => {
      const script = document.createElement('script'); script.src = '/artifact-editor.js';
      script.onload = resolve; script.onerror = () => { editorScript = null; reject(Error('Editor unavailable. Reopen Files to retry.')); };
      document.head.append(script);
    });
  }
  function open(source, container) {
    const dialog = document.createElement(container ? 'section' : 'dialog');
    dialog.className = 'file-explorer'; dialog.setAttribute('aria-label', source.title);
    dialog.innerHTML = `<div class="fx-header"><div><small></small><h2></h2></div><button type="button" class="button small" data-close>Close</button></div>
      <div class="fx-body"><aside class="fx-nav"><input type="search" placeholder="Find a file…" aria-label="Find a file">
      <nav aria-label="Folder path"></nav><div class="fx-list" aria-label="Files"></div></aside>
      <section class="fx-document"><header><div><h3></h3><p></p></div></header><div class="fx-paper"></div>
      <footer><span role="status"></span><div class="fx-actions"></div></footer></section></div>`;
    const $ = selector => dialog.querySelector(selector);
    $('.fx-header small').textContent = source.scope;
    $('.fx-header h2').textContent = source.title;
    const paper = $('.fx-paper'), status = $('[role=status]'), list = $('.fx-list'), actions = $('.fx-actions');
    let folder = '', selected, editor, original = '', sequence = 0, busy = false, limit = 100, ready = false;
    const button = (label, fn, cls='button small') => {
      const b = document.createElement('button'); b.type = 'button'; b.className = cls; b.textContent = label; b.onclick = fn; return b;
    };
    // Keep full-document comparisons off the typing path, even for megabyte pages.
    let changed = false, settle;
    const dirty = () => changed && editor && editor.content() !== original;
    function edited() {
      changed = true; notice('Unsaved changes'); controls();
      clearTimeout(settle); settle = setTimeout(() => {
        if (changed && !dirty()) { changed = false; notice(''); controls(); }
      }, 350);
    }

    const discard = () => !dirty() || confirm('Discard unsaved changes to ' + selected.path + '?');
    const error = e => { status.textContent = e.message; status.classList.add('fx-error'); };
    const notice = message => { status.classList.remove('fx-error'); status.textContent = message; };
    function close() { if (!busy && discard()) dialog.close(); }
    if (container) $('.fx-header').remove();
    else $('[data-close]').onclick = close;
    dialog.oncancel = e => { e.preventDefault(); close(); };
    const unload = e => { if (dirty()) { e.preventDefault(); e.returnValue = ''; } };
    window.addEventListener('beforeunload', unload);
    const destroy = () => { ++sequence; clearTimeout(settle); editor?.destroy(); window.removeEventListener('beforeunload', unload); dialog.remove(); };
    dialog.onclose = destroy;
    function controls() {
      for (const b of actions.children) b.disabled = busy || (b.dataset.save !== undefined ? !changed : changed);
      if (!container) $('[data-close]').disabled = busy;
    }
    function drawList() {
      const focused = list.contains(document.activeElement) ? document.activeElement.title : null;
      const query = $('input').value.trim().toLowerCase(), items = new Map(), prefix = folder ? folder+'/' : '';
      const nav = $('nav'); nav.replaceChildren();
      const jump = path => { folder = path; $('input').value = ''; limit = 100; drawList(); };
      nav.append(button(source.root, () => jump(''), 'fx-crumb'));
      let here = '';
      for (const part of folder.split('/').filter(Boolean)) {
        here += (here ? '/' : '') + part; const path = here;
        nav.append(document.createTextNode(' / '), button(part, () => jump(path), 'fx-crumb'));
      }
      for (const f of source.files) {
        if (query) {
          if ((f.path+' '+(f.detail||'')).toLowerCase().includes(query)) items.set(f.path,{name:f.path,file:f});
        } else if (f.path.startsWith(prefix)) {
          const rest = f.path.slice(prefix.length), slash = rest.indexOf('/');
          if (slash === -1) items.set(rest,{name:rest,file:f});
          else { const name = rest.slice(0,slash); items.set(name,{name,folder:prefix+name}); }
        }
      }
      const rows = [...items.values()].sort((a,b)=>Number(!!b.folder)-Number(!!a.folder)||a.name.localeCompare(b.name));
      const fragment = document.createDocumentFragment();
      for (const item of rows.slice(0,limit)) {
        const b = button('',()=>item.file ? select(item.file) : jump(item.folder),'fx-row');
        const mark = document.createElement('span'); mark.innerHTML = HeyBossUI.icon(item.file?'docs':'folder');
        const name = document.createElement('span'); name.textContent = item.name; b.title = item.name; b.append(mark,name);
        if (item.file?.path === selected?.path && item.file) b.setAttribute('aria-current','true');
        fragment.append(b);
      }
      if (rows.length > limit) fragment.append(button('Show more files',()=>{limit+=100;drawList();}));
      if (!rows.length) { const empty = document.createElement('p'); empty.textContent = 'No matching files'; fragment.append(empty); }
      list.replaceChildren(fragment);
      if (focused !== null) ([...list.children].find(b=>b.title===focused) || list.querySelector('button'))?.focus();
    }
    $('input').oninput = () => {limit=100;drawList();};
    async function run(fn) {
      if (busy) return;
      busy = true; editor?.readOnly(true); controls(); notice('Working…');
      try { await fn(); } catch(e) { error(e); }
      finally { busy=false; editor?.readOnly(false); controls(); }
    }
    async function select(file, force=false) {
      if (!force && (busy || file === selected || !discard())) return;
      const seq = ++sequence; clearTimeout(settle); changed=false;
      editor?.destroy(); editor=null; ready=false; selected=file; actions.replaceChildren(); paper.replaceChildren(); drawList();
      $('h3').textContent = file?.path || 'Select a file'; $('.fx-document header p').textContent = file?.detail || '';
      notice(file ? 'Opening…' : '');
      if (!file) return;
      source.onSelect?.(file);
      try {
        if (file.editable) {
          const [text] = await Promise.all([source.read(file),loadEditor()]);
          if (seq !== sequence) return;
          const host=document.createElement('div'); paper.append(host);
          editor=HeyBossArtifactEditor(host,text,edited);
          original=editor.content(); ready=true;
          const save = button(source.saveLabel || 'Save',()=>run(async()=>{
            await source.save(file,editor.content()); original=editor.content(); changed=false;
            $('.fx-document header p').textContent=file.detail||''; buildActions(); notice('Saved');
          }),'button primary small'); save.dataset.save=''; fileSave=save;
        } else {
          const a=document.createElement('a'); a.textContent='Open original file'; a.href=source.url(file); a.target='_blank'; a.rel='noopener noreferrer';
          paper.append(a);
        }
        buildActions(); notice('');
      } catch(e) { if(seq===sequence) { buildActions(); error(e); } }
    }
    let fileSave;
    function buildActions() {
      actions.replaceChildren(); const file=selected;
      if (source.native && ready) actions.append(button('Open in Mac editor',()=>run(async()=>{
        await source.native(file); if (container) notice('Opened in Mac editor'); else dialog.close();
      })));
      if (source.reset && file.custom) actions.append(button('Restore default…',()=>{
        if(confirm('Restore the built-in version of '+file.path+'?')) run(async()=>{
          await source.reset(file); await select(file,true); notice('Default restored');
        });
      }));
      if (source.move && file.path!=='index.html') {
        actions.append(button('Move…',()=>moveDialog(file)),button('Delete…',()=>{
          if(confirm('Delete '+file.path+' from this published page on all machines?')) run(async()=>{
            await source.remove(file); folder=''; $('input').value=''; await select(null,true); notice('File deleted');
          });
        }));
      }
      if (ready) actions.append(fileSave);
      controls();
    }
    function moveDialog(file) {
      const move = document.createElement('dialog'); move.className='fx-move'; move.setAttribute('aria-label','Move file');
      move.innerHTML='<form><h3>Move file</h3><label>Destination path<input required autocomplete="off" spellcheck="false" maxlength="1024"></label><p>Include the filename. Use a slash to move it into a folder.</p><div><button class="button" type="button">Cancel</button><button class="button primary" type="submit">Move file</button></div></form>';
      const input=move.querySelector('input'); input.value=file.path;
      move.querySelector('[type=button]').onclick=()=>move.close();
      move.onclose=()=>move.remove();
      move.querySelector('form').onsubmit=e=>{
        e.preventDefault(); const destination=input.value; move.close();
        if(destination!==file.path) run(async()=>{
          await source.move(file,destination);
          folder=destination.includes('/')?destination.slice(0,destination.lastIndexOf('/')):'';
          $('input').value=''; await select(file,true); notice('Moved');
        });
      };
      document.body.append(move); move.showModal(); input.select();
    }
    dialog.onkeydown = e => {
      if ((e.metaKey||e.ctrlKey) && e.key.toLowerCase()==='s') { e.preventDefault(); actions.querySelector('[data-save]')?.click(); }
      if(e.key==='ArrowDown'||e.key==='ArrowUp') {
        const rows=[...list.querySelectorAll('button')], i=rows.indexOf(document.activeElement);
        if(i>=0){e.preventDefault();rows[Math.max(0,Math.min(rows.length-1,i+(e.key==='ArrowDown'?1:-1)))].focus();}
      }
    };
    if (container) container.replaceChildren(dialog);
    else { document.body.append(dialog); dialog.showModal(); $('input').focus(); }
    select(source.files.find(f=>f.path===source.initial) || source.files[0]);
    return () => { if (busy || !discard()) return false; destroy(); return true; };
  }
  return {open};
})();
