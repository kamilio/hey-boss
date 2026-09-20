import {build} from 'esbuild';
import {existsSync,readFileSync,appendFileSync} from 'node:fs';
import {dirname,resolve,join} from 'node:path';
const outfile=new URL('../src/issues/web/artifact-editor.js',import.meta.url).pathname;
const result=await build({entryPoints:[new URL('./artifact-editor.mjs',import.meta.url).pathname],outfile,bundle:true,minify:true,format:'iife',target:['safari16','chrome100'],legalComments:'eof',metafile:true});
const packages=new Set();
for(const input of Object.keys(result.metafile.inputs)){
 if(!input.includes('node_modules/'))continue;
 let directory=dirname(resolve(input));
 while(!existsSync(join(directory,'package.json')))directory=dirname(directory);
 packages.add(directory);
}
for(const directory of [...packages].sort()){
 const info=JSON.parse(readFileSync(join(directory,'package.json'),'utf8'));
 const license=['LICENSE','LICENSE.md','LICENSE.txt','license'].map(name=>join(directory,name)).find(existsSync);
 if(!license)throw Error('Missing bundled license: '+info.name);
 appendFileSync(outfile,'\n/*! '+info.name+' '+info.version+'\n'+readFileSync(license,'utf8').replaceAll('*/','* /')+' */\n');
}
