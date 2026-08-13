# Signed application updates

RemoteX 1.2 and newer use the official Tauri updater with a GitHub Release
manifest. Tauri verifies every updater artifact with the public key embedded in
`apps/desktop/tauri.conf.json`; signature verification cannot be disabled.

## Key custody

The updater private key is deliberately outside this repository. On the release
maintainer's Windows machine it is stored at:

```text
C:\Users\zhang\.remotex\updater.key
```

The directory inherits no ACLs and grants access only to the current user. The
same key is stored in the repository's `TAURI_SIGNING_PRIVATE_KEY` GitHub Actions
secret. Back up the private key to a separate encrypted, access-controlled
location. Losing it permanently prevents installed copies from accepting future
updates. Never commit, print, paste, or send the private key.

## Local signed build

In a PowerShell process, point Tauri at the private key before building:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -Raw 'C:\Users\zhang\.remotex\updater.key'
.\packaging\windows\build-installer.ps1
```

The build must produce both `RemoteX_*_x64-setup.exe` and the adjacent
`RemoteX_*_x64-setup.exe.sig` file.

## Release contract

Pushing a `v*` tag runs `.github/workflows/release.yml`. The workflow:

1. signs the current-user NSIS installer with the protected updater key;
2. uploads the installer, signature, and SHA-256 checksum;
3. creates `latest.json` with the exact signature contents and download URL;
4. publishes all Windows and Linux artifacts in the GitHub Release.

Before tagging, ensure the tag version matches the workspace and Tauri versions.
After publishing, verify that `latest.json`, the installer, and its `.sig` URL all
return successfully. Do not delete or replace an updater asset in an existing
release; publish a higher semantic version instead.

The updater signing key authenticates Tauri updates. It does not remove the
Windows unknown-publisher warning; that requires a separate trusted Authenticode
code-signing certificate.
