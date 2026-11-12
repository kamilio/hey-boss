import { beforeEach, vi } from 'vitest';
import { parse } from './upstream/packages/toolcraft-design/src/terminal-markdown/parser.js';

function snapshot(value: unknown) {
  const parents: object[] = [];
  return JSON.parse(JSON.stringify(value, function (_, item) {
    if (item && typeof item === 'object') {
      while (parents.length && parents.at(-1) !== this) parents.pop();
      if (parents.includes(item)) return '[Circular]';
      parents.push(item);
    }
    return item;
  }));
}

if (process.env.POE_MARKDOWN_CAPTURE) {
  const modules = import.meta.glob('./upstream/packages/toolcraft-design/src/terminal-markdown/{index,parser,plaintext-renderer,demo-content,parser/{block,inline,frontmatter,code-highlight}}.ts');
  let calls: unknown[];
  let inside = false;
  beforeEach(({ task }) => { task.meta.calls = calls = []; });

  for (const load of Object.values(modules)) {
    const module = await load() as Record<string, (...args: any[]) => any>;
    for (const [operation, original] of Object.entries(module)) {
      if (typeof original !== 'function') continue;
      vi.spyOn(module, operation).mockImplementation((...args) => {
        // Keep calls made by the test; exclude the implementation's own calls.
        if (inside || !calls) return original(...args);
        inside = true;
        try {
          const result = original(...args);
          const first = args[0];
          const source = operation === 'getMarkdownDemo' ? result : typeof first === 'string' ? first : undefined;
          const ast = source !== undefined ? (operation === 'parse' ? result : parse(source)).ast : first?.type ? first : undefined;
          calls.push(snapshot(operation === 'highlightCodeBlock' ? { operation, code: first, result } : { operation, source, ast }));
          return result;
        } finally {
          inside = false;
        }
      });
    }
  }
}
