- “Conversations” means the Hey Boss desktop conversation chat. Start with `VoiceConversation` in `hey_boss_daemon.swift`; its end-to-end checks are in `tools/conversation_checks.sh`.
- No PRs, straight to main
- Commit small changes
- Run GitHub Actions only when the user explicitly asks. Run tests locally on demand; use your judgment about what each change needs.
- Attention to performance
- Keep the design slick and polished.
- No useless params and config, it's easier to change code. 
- Install changed components only on machines that use them. For Hey Talk–only changes, use `packages/hey-talk/install.sh` on Macs with Hey Talk; skip `hey-boss upgrade`.
- Use Rust if possible, NO PYTHON
- The user writes prompts. Request prompt wording through `hey-boss notif ask`; do not invent or expand it.
- Store all prompt text in Markdown (`.md`) files. Code must load prompts from those files, never embed them.

## Prompt style

- Index-card length. One-liners.
- Only non-obvious guidance that changes a decision.
- A few concrete examples; leave command details to `--help`.
- Link advanced details in references; read only when needed.
- Preserve the user's cuts; don't grow the manual back.

- Keep desktop and iPhone Safari/Home Screen Conversations in parity. Use `VoiceConversation` for chat state, turn detection, transcription, playback and controls; `src/conversation_web/` handles phone input and presentation. For a change to either client, exercise the same turn on both.
