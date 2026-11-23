import mermaid from 'mermaid';
window.renderDiagrams = async () => {
  const dark = matchMedia('(prefers-color-scheme: dark)').matches;
  mermaid.initialize({startOnLoad:false, securityLevel:'strict', suppressErrorRendering:true,
    maxTextSize:50000, maxEdges:500, htmlLabels:false, theme:dark?'dark':'neutral',
    fontFamily:'-apple-system, sans-serif', flowchart:{htmlLabels:false},
    secure:['securityLevel','startOnLoad','suppressErrorRendering','maxTextSize','maxEdges','htmlLabels','flowchart','theme','themeVariables','themeCSS']});
  let index = 0;
  const errors = [];
  for (const code of document.querySelectorAll('pre > code.language-mermaid')) {
    const stage = document.createElement('div');
    document.body.append(stage);
    try {
      const {svg} = await mermaid.render('sticky-diagram-' + index++, code.textContent, stage);
      const figure = document.createElement('figure');
      figure.className = 'diagram'; figure.innerHTML = svg;
      figure.setAttribute('aria-label', 'Diagram');
      code.parentElement.replaceWith(figure);
    } catch (error) {
      errors.push(String(error));
      const message = document.createElement('p');
      message.className = 'error'; message.textContent = 'Could not render this diagram. Check its Mermaid source.';
      code.parentElement.before(message);
    } finally { stage.remove(); }
  }
  return errors;
};
