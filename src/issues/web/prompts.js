"use strict";
(async () => {
  const container = document.getElementById('prompt-browser');
  let boot, unmount, currentHash, generation = 0;
  const picker = new HeyBossUI.ProjectPicker({onSelect(project) {
    const params = new URLSearchParams(location.hash.slice(1));
    params.set('project', project); params.delete('file');
    location.hash = params;
  }});
  HeyBossUI.icons();
  async function navigate() {
    if (unmount && !unmount()) { history.replaceState(null, '', currentHash); return; }
    unmount = null;
    const sequence = ++generation;
    const status = document.createElement('p');
    status.setAttribute('role', 'status'); status.textContent = 'Loading prompt files…';
    container.replaceChildren(status);
    try {
      const project = HeyBossUI.projectId(boot.project?.id || boot.projects[0]?.id);
      const params = new URLSearchParams(location.hash.slice(1));
      const host = params.get('host') || boot.backend_host || null;
      if (!project) { status.textContent = 'Choose a project to view its prompts.'; return; }
      picker.update(boot.projects, boot.projects.find(p => p.id === project) || {id: project, name: project});
      currentHash = location.hash;
      const action = (operation, request_id = null) =>
        HeyBossUI.post('/api/action', {project, host, operation, request_id}, boot);
      let settings = await action({action:'project_settings'});
      if (sequence !== generation) return;
      const files = [{key:'worker', title:'Implementation'}, ...settings.prompt_sections].map(section =>
        ({...section, path:section.key+'.md', editable:true}));
      function describe(file) {
        file.custom = file.key === 'worker' ? settings.prompt !== settings.worker_prompt_default : settings.prompt_overrides[file.key] != null;
        file.detail = [file.title, file.custom ? 'Project override' : 'Built-in default', file.help].filter(Boolean).join(' · ');
      }
      files.forEach(describe);
      async function save(file, text) {
        const patch = file.key === 'worker' ? {prompt:text ?? settings.worker_prompt_default}
          : {prompt_overrides:{...settings.prompt_overrides, [file.key]:text}};
        settings = await action({action:'configure_project', if_version:settings.version, ...patch}, HeyBossUI.requestId());
        describe(file);
      }
      unmount = HeyBossFiles.open({title:'Prompt files', root:'prompts', files, initial:params.get('file') || 'worker.md',
        onSelect(file) {
          const route = new URLSearchParams(location.hash.slice(1));
          route.set('file', file.path); currentHash = '#' + route;
          history.replaceState(null, '', currentHash);
        },
        read:file => file.key === 'worker' ? settings.prompt : settings.prompt_overrides[file.key] ?? settings.prompt_defaults[file.key],
        save, reset:file => save(file, null),
        native:!host && ['127.0.0.1','localhost','hey-boss.test'].includes(location.hostname) && /Mac/.test(navigator.platform)
          ? file => HeyBossUI.post('/api/prompts/open', {project, key:file.key}, boot) : null
      }, container);
    } catch (error) {
      if (sequence !== generation) return;
      status.setAttribute('role','alert'); status.textContent = error.message;
      const retry = document.createElement('button'); retry.className = 'button';
      retry.textContent = 'Retry'; retry.onclick = navigate; container.append(retry);
    }
  }
  try {
    const response = await fetch('/api/bootstrap', {signal:AbortSignal.timeout(10000)});
    boot = await response.json();
    if (!response.ok || !boot.ok) throw Error(boot.error?.message || 'Unable to connect.');
    window.addEventListener('hashchange', navigate);
    document.querySelector('#nav-prompts').scrollIntoView({block:'nearest', inline:'nearest'});
    await navigate();
  } catch (error) { container.textContent = error.message; }
})();
