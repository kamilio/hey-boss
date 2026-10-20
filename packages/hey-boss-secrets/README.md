# Named secrets for local tools

`hey-boss secrets` stores raw bytes, JSON bundles, and cookie files. It supplies credentials to official libraries and CLIs. It does not proxy requests, refresh OAuth tokens, or sign in to providers.

## Setup

The installer runs `hey-boss secrets install`: a private machine identity goes in macOS Keychain or Linux pass. A Linux machine without a pass store gets its own passphrase-free GPG key. Existing stores and keys are preserved. Python and JavaScript SDK sources are installed at `~/.local/share/hey-boss/sdk/secrets/`.

`hey-boss secrets enroll HOST` exchanges signed public cards over an already trusted SSH host. It initializes the destination if needed. Enrollment establishes trust; it does **not** grant records. `hey-boss secrets grant NAME MACHINE_ID` grants read/write access to one record. Only that record's original owner can grant or revoke.

Import a field with `hey-boss secrets import NAME --reference op://Agents/ITEM/FIELD`. This uses the existing `~/.hey-proxy/op-agent` identity. An optional `--keychain-service SERVICE --keychain-account ACCOUNT` is tried if 1Password is unavailable. No provider is contacted after import. Nothing is written back to 1Password automatically.

Use `put NAME --file PATH --kind bundle` for first-party credential JSON, or pipe bytes into `put`. `get NAME --file PATH` writes atomically with mode 0600. `get NAME --stdout` requires a pipe. Values are never command-line arguments. Do not pipe values into logs. All default command output is metadata. Secret names and machine labels are public metadata: do not put credentials in names.

## Replication and conflicts

Each immutable journal entry contains a signed policy, signed writer identity, timestamps, and age-encrypted content. Ed25519 authenticates revisions; age X25519 encrypts a fresh payload for the explicitly granted machines. The existing cr-sqlite journal distributes those entries. Plaintext never enters a replicated database. The local index retains winning encrypted revisions only; history stays in the journal.

Any granted machine can read and write locally with every other machine offline. Surviving machines exchange updates over existing direct fleet peer routes. Configure reachable `sync_address` entries for those machines; a star with only a controller cannot replicate while its controller is down. Replication resumes automatically after a partition. Losing all links means local operation continues, but updates cannot travel until a link returns.

For the same access policy, the largest `(wall-clock milliseconds, logical counter, writer ID, revision hash)` wins. The logical counter advances when a write observes an equal/later clock, so the next observed write wins even when the local clock is behind. Equal timestamps resolve deterministically, independent of delivery order. Whole bundles/blobs are replaced, never field-merged. Keep clocks reasonably synchronized: disconnected machines cannot know real global write order.

Access policy version takes precedence over data time. A revoked machine cannot undo revocation with a newer timestamp under its old policy. Access changes require the owner's signing key; **data access does not depend on the owner being online**. Losing the owner permanently requires recovery from its local identity backup or creating a new record from a surviving granted copy. This first version has no automatic ownership recovery.

Independently created records with the same name and different trusted owners remain distinct. Address them by record ID if a name is ambiguous. `--expected REVISION` (or SDK `expected`) provides optional stale-write detection; normal updates use latest-wins.

Revocation cannot erase copies already delivered, old encrypted journal entries, or a credential's validity at Google. Rotate a provider credential after revoking a machine. Offline machines learn grants, revocations, and deletions only on reconnection. Deletions are signed tombstones. Deleted records cannot currently be recreated under the same owner/name.

## SDKs

Rust: depend on the `hey-boss-secrets` workspace crate. `client::Client::default().get("google/personal")` returns `(Record, Secret)`. `Secret::expose()` returns bytes. `put(name, bytes, "bundle", Some(&record.revision))` saves changed bytes.

Python: install `~/.local/share/hey-boss/sdk/secrets/python` with pip, or add it to the tool's module path.

```python
from hey_boss_secrets import Client
store = Client()
record, secret = store.get("google/personal")
# Give secret.expose() to the official Google credentials library.
# After that library refreshes, save its updated credential JSON:
# store.put(record["id"], updated_json_bytes, kind="bundle", expected=record["revision"])
```

JavaScript: install `~/.local/share/hey-boss/sdk/secrets/javascript` with npm, or import its `index.js` directly.

```js
import { Client } from '@hey-boss/secrets';
const store = new Client();
const { record, secret } = await store.get('google/personal');
// Pass secret.expose() to the official client, then save updated bytes:
// await store.put(record.id, updatedBytes, {kind: 'bundle', expected: record.revision});
```

All three SDKs use the local CLI over private pipes, not a daemon. Secret wrappers redact normal printing/inspection; exposing or parsing bytes hands responsibility to the caller. Managed runtimes may retain copies in memory. Errors use fixed codes and discard subprocess diagnostics. For tools that require files, create an owner-only temporary directory, export there, let the official tool refresh its file, then write it back and remove the temporary file. Do not keep credentials in skill folders.

## Local security boundary

This unattended model trusts processes running as the machine's OS user. Linux private keys have no passphrase, as explicitly chosen for this version. Mac Keychain access runs through Apple's signed `osascript` using a fixed bundled Security API script, so an ad-hoc hey-boss upgrade does not invalidate its Keychain identity. The script disables all Keychain interaction. A locked or unavailable Keychain fails closed. No plaintext backend fallback exists. Hardware-backed identity keys and user-presence checks are outside this version.

`/secrets` and `/api/secrets` show metadata only. They never read Keychain/pass values, expose ciphertext, or provide a reveal operation. “Granted” reports the access policy, not whether the local Keychain is currently unlocked.
