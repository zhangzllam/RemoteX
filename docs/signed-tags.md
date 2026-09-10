# Signed release tags

Every published release must use an annotated, cryptographically signed Git
tag whose version exactly matches Cargo, Tauri, the UI package, and artifacts.

One-time maintainer setup:

1. Create or select a protected GPG release-signing key.
2. Add its armored public key as the `RELEASE_SIGNING_PUBLIC_KEY` GitHub Actions
   secret.
3. Protect the release branch and restrict tag creation to maintainers.

Create a release tag only after the v1.2 checklist is complete:

```bash
git tag -s v1.2.0 -m "RemoteX v1.2.0"
git verify-tag v1.2.0
git push origin v1.2.0
```

The release validation job imports the configured public key, rejects a
lightweight tag, runs `git verify-tag`, and rejects any tag that does not equal
`v` plus the workspace version. Windows and Linux artifact jobs depend on this
validation; publication depends on both artifact jobs.

Do not reuse, move, or replace a published release tag. Revoke and rotate a
compromised key, document the incident, and publish a new patch version.

