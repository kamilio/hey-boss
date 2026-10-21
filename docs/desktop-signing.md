# Desktop signing

Hey Boss shares Hey Talk's persistent local signing certificate, pinned in `assets/signing-identity.sha1`. Restore it on each Mac that builds the desktop app with `packages/hey-talk/setup-signing.sh`; the private identity is backed up in 1Password. Packaging refuses ad-hoc fallback. Installed apps do not need the private key.

The first switch from ad-hoc signing may require a new permission approval. Updates retain the certificate and stable CLI/desktop identifiers. Launch agents associate with the installed desktop app; upgrades refresh Launch Services and existing registrations without resetting permissions. CLI-only updates preserve the desktop bundle when its source fingerprint and signature match.

Local certificates preserve code requirements, but do not guarantee Local Network permission retention. Apple recommends an Apple-issued identity for that subsystem ([TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)). App Management is separate: macOS can attribute a worker's child browser to Hey Boss. Check the responsible app in system logs before changing any grant.

Run `bash tools/test_hey_boss_signing.sh` to verify changed builds keep the same identity and an ad-hoc replacement cannot satisfy it.
