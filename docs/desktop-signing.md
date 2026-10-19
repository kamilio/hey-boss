# Desktop signing

Hey Boss shares Hey Talk's persistent local signing certificate, pinned in `assets/signing-identity.sha1`. Restore it on each Mac that builds the desktop app with `packages/hey-talk/setup-signing.sh`; the private identity is backed up in 1Password. Packaging refuses ad-hoc fallback. Installed apps do not need the private key.

The first switch from ad-hoc signing may require one final permission approval. Later updates retain the certificate and `local.hey-boss.desktop` identity. Do not reset permissions during installation.

Run `bash tools/test_hey_boss_signing.sh` to verify changed builds keep the same identity and an ad-hoc replacement cannot satisfy it.
