import {defineConfig} from 'vitest/config';
const observed = new Set(['parse','parseInline','parseBlocks','extractFrontmatter','render','renderHtml','renderMarkdown','renderMarkdownHtml','renderPlaintext','renderMarkdownPlaintext','highlightCodeBlock','getMarkdownDemo']);
export default defineConfig({
  plugins:[{name:'capture-upstream-inputs',enforce:'pre',transform(code,id){
    if (!process.env.POE_MARKDOWN_CAPTURE || !id.endsWith('.test.ts')) return;
    const wrappers=[];
    code=code.replace(/import\s*\{([^}]+)\}\s*from\s*(["'][^"']+["']);/g,(original,names,from)=>{
      const replacements=names.split(',').map(name=>{
        name=name.trim();
        if(!observed.has(name)) return name;
        wrappers.push(`const ${name} = (...args: any[]) => { const result = __original_${name}(...args); (globalThis as any).__poeCapture(${JSON.stringify(name)}, args, result); return result; };`);
        return `${name} as __original_${name}`;
      });
      return `import { ${replacements.join(', ')} } from ${from};`;
    });
    return wrappers.join('\n')+'\n'+code;
  }}],
  test:{include:['upstream/**/*.test.ts'],setupFiles:['./capture.ts'],testTimeout:30000,maxWorkers:1}
});
