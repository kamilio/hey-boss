# Codex storage

Hey-boss keeps tasks and worker state in its own database. Its observers never open Codex's `state_*.sqlite`, including read-only connections, and never change Codex's SQLite settings, journals, or locks.

Session titles come from a bounded read of `session_index.jsonl`. Resume targets, activity, saved repository metadata, and creator models come from the matching session JSONL file, including archived sessions. Missing metadata stays unknown; it never triggers a database fallback. Creator models outside the bounded transcript tail are not guessed from older turns.

Managed Codex processes still use Codex's own storage through the normal Codex runtime. This separation removes hey-boss's direct SQLite access; it does not disable Codex's own database or change independent sessions.
