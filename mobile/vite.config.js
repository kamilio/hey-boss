import {defineConfig} from 'vite';
import {fileURLToPath} from 'node:url';
export default defineConfig({
 server:{proxy:{'/api':'http://127.0.0.1:8787'}},build:{sourcemap:false},
 // Entity decoding has a DOM-specific browser export. Workers need its pure
 // lookup-table implementation; keep the smaller browser export for the page.
 worker:{plugins:()=>[{name:'worker-entity-decoding',enforce:'pre',resolveId(id){if(id==='decode-named-character-reference')return fileURLToPath(new URL('./node_modules/decode-named-character-reference/index.js',import.meta.url));}}]},
});
