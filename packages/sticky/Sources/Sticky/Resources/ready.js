(async () => {
  const errors = window.renderDiagrams ? await window.renderDiagrams() : [];
  await document.fonts.ready;
  await Promise.all([...document.images].map(image => new Promise(resolve => {
    const finished = () => { if (!image.naturalWidth) errors.push('Could not load image: ' + image.alt); resolve(); };
    if (image.complete) finished(); else { image.onload = finished; image.onerror = finished; }
  })));
  window.webkit.messageHandlers.ready.postMessage({errors, height:Math.ceil(document.body.getBoundingClientRect().height)});
})();
