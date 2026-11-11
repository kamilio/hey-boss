// Render the fixtures with the packaged agent, then exercise its native viewer.
import {readFile, writeFile} from 'node:fs/promises';
import {resolve, join} from 'node:path';
import {pathToFileURL} from 'node:url';
const [resources, folder] = process.argv.slice(2);
const {prepareMedia} = await import(pathToFileURL(resolve(resources, 'conversation-media.mjs')));
const icon = await readFile(new URL('../assets/hey-boss.icns', import.meta.url));
const sources = [
  {title: 'From draft to release', graphviz: 'digraph { rankdir=LR; Draft -> Review -> Release; Review -> Revise; Revise -> Review }'},
  {title: 'A conversation turn', mermaid: 'sequenceDiagram\n  participant You\n  participant Agent\n  participant Canvas\n  You->>Agent: Ask a question\n  Agent->>Canvas: Show diagram\n  Agent-->>You: Explain it'},
  {title: 'The desktop app', image: {type: 'image', mimeType: 'image/icns', data: icon.toString('base64')}},
  {title: 'Where the time goes', svg: '<svg xmlns="http://www.w3.org/2000/svg" width="460" height="180" viewBox="0 0 460 180"><g font-family="Helvetica" font-size="15" fill="#273d56"><text x="8" y="40">Explore</text><text x="8" y="90">Build</text><text x="8" y="140">Review</text></g><g fill="#377f83"><rect x="94" y="22" width="132" height="24" rx="5"/><rect x="94" y="72" width="300" height="24" rx="5"/><rect x="94" y="122" width="84" height="24" rx="5"/></g></svg>'},
  {title: 'From field to shelf', graphviz: 'digraph { Seed -> Grow -> Harvest -> Inspect -> Sort -> Grade -> Pack -> Seal -> Label -> Dispatch -> Receive -> Stock }'},
  {title: 'The delivery pipeline', graphviz: 'digraph { rankdir=LR; Discover -> Design -> Prototype -> Implement -> Verify -> Package -> Release -> Observe -> Improve }'},
];
const fixtures = [];
for (const source of sources) fixtures.push(await prepareMedia(source, folder, new AbortController().signal));
await writeFile(join(folder, 'media.json'), JSON.stringify(fixtures));
