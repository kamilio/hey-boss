- Conversations on desktop and iPhone Safari/Home Screen share `VoiceConversation` in `hey_boss_daemon.swift`. Match the native layout, colors, controls, and behavior; `src/conversation_web/` handles phone input and presentation using the shared Hey Boss web components in `src/issues/web/components.css`.
- Run `tools/conversation_checks.sh <cli> <audit-daemon>` for desktop. For the phone UI, open the Conversations URL with `playwright-cli -s=chats open <url> --browser webkit`. Run each of `tools/conversation_safari_checks.js` and `tools/conversation_recovery_checks.js` with `playwright-cli -s=chats run-code --filename <file>`, then close the session.
- Offline text queues must survive reloads, remain tied to their chat, and retry without duplicate messages. Run `PLAYWRIGHT_CORE=<playwright-core path> node tools/conversation_browser_checks.js` for WebKit/Chrome voice, recovery, and offline checks.
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
