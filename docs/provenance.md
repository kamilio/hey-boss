# Creation origins

Issues, subtasks, drafts, and artifacts automatically save their creator's
session, device, checkout, and creating tool invocation when available. No new
CLI options are required. The context is captured on the caller's device before
SSH transport; a follow-up in another project still points to its source task.

The **Origin** card opens the creator's saved conversation. When an invocation
was recorded, it loads surrounding context and expands and highlights the
creating tool call. **Created in this run** links back to issues and artifacts
from that attempt. Standalone Codex sessions have equivalent session links.
Historical origin links work after an attempt leaves the recent Agents list.

JSON issue and artifact details expose `origin`, including `actor_id`,
`session_id`, `machine`, `host`, `cwd`, `source`, `created_at`, `run`, and
`invocation`. Invocation metadata contains a transcript byte offset and optional
tool call ID. It does not copy tool arguments, outputs, or private reasoning.

Origins are immutable through edits, retries, reopening, and project transfers.
Issue origins travel with fleet replicas, including offline creation. Artifact
origins stay in the authoritative artifact store. Legacy rows retain an unknown
origin rather than guessing from the latest run. Human creations never inherit
the launching agent's session.

Capturing invocation metadata is best effort and reads at most the last 8 MiB of
the exact Codex session's transcript. Missing metadata never prevents creation.
Conversation viewing still requires the owning device and saved transcript;
disconnected devices report that explicitly. Claude session identity is retained,
but its transcript viewer is not currently supported. History and relationships
exclude hidden projects. Resource lists are bounded to 50 issues and 50 artifacts.
