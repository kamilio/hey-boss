- No PRs, straight to main
- Commit small changes
- TDD
- Attention to performance
- Every change must have a slick, polished design and be verified end-to-end before it is considered complete.
- No useless params and config, it's easier to change code. 
- After finishing task, install on all machines, deploy fly
- Use Rust if possible, NO PYTHON
- The user writes prompts. Request prompt wording through `hey-boss notif ask`; do not invent or expand it.
- Store all prompt text in Markdown (`.md`) files. Code must load prompts from those files, never embed them.

## Prompt style

- Index-card length. One-liners.
- Only non-obvious guidance that changes a decision.
- A few concrete examples; leave command details to `--help`.
- Link advanced details in references; read only when needed.
- Preserve the user's cuts; don't grow the manual back.
