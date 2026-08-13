# RemoteX

RemoteX is a self-hosted remote-control system for visibly and explicitly
accessing your own Windows PCs and Linux servers. The project includes a Windows
desktop application, a headless Linux component, an HTTPS Control Server, and
an end-to-end encrypted QUIC Relay.

## Capabilities

- Windows desktop capture with cursor composition, monitor selection, adaptive
  H.264/JPEG video, mouse, keyboard, clipboard, and resumable file transfer.
- Foreground accept/reject by default, independent per-session permissions,
  visible active-session state, local disconnect, and opt-in unattended access.
- Linux PTY terminals, safe rooted file transfer, and read-only system metrics.
- Ed25519 device enrollment, nine-digit Device IDs, signed presence, one-time
  role-bound credentials, and short-lived session material.
- Mutually authenticated direct QUIC attempts for explicitly advertised
  candidates, with automatic fallback to the Relay.
- XChaCha20-Poly1305 payload encryption above QUIC/TLS. The Relay forwards
  ciphertext and cannot read frames, input, clipboard, terminal, or file data.
- Docker Compose self-hosting with PostgreSQL, Caddy TLS, secret files,
  health/readiness probes, JSON logs, and unprivileged application containers.

## Install and run

Download the Windows NSIS installer or Linux Agent archive from the latest
[GitHub Release](https://github.com/zhangzllam/RemoteX/releases). The Windows
installer is current-user scoped. Windows may show an unknown-publisher warning
until a trusted code-signing certificate is configured.

Current RemoteX builds can check and download cryptographically signed updates in
the background from **Settings > General**, then install them from **About**.
Installing waits until active remote sessions have ended. Users on 1.1 or older
must install 1.2 manually once because those builds do not contain the updater.

For a production deployment, follow [Running M13](docs/running-m13.md). For the
Windows packaged application, follow [Running M14](docs/running-m14.md). Linux
Agent environment variables and systemd setup are covered in
[Running M12](docs/running-m12.md). Maintainers should also read the
[signed update guide](docs/signed-updates.md). Release maintainers should also
read [signed tags](docs/signed-tags.md), [Windows signing](docs/windows-signing.md),
and the [v1.2 release checklist](docs/v1.2-release-checklist.md).

## Development

RemoteX requires Rust 1.88 or newer. The desktop frontend also requires Node.js
22 and pnpm 11.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
pnpm --dir apps/desktop/ui install --frozen-lockfile
pnpm --dir apps/desktop/ui build
pnpm --dir apps/desktop/ui benchmark:video
cargo audit
pnpm --dir apps/desktop/ui audit --prod
```

Build the Windows installer from an MSVC PowerShell session:

```powershell
./packaging/windows/build-installer.ps1
```

## Architecture and security

The workspace separates shared protocol/capability code in `crates/`, clients
in `apps/`, and network services in `servers/`. See [Architecture](docs/architecture.md),
[Protocol](docs/protocol.md), [Security review](docs/security-review.md), and the
[V1 release checklist](docs/v1-release-checklist.md).

Interactive approval is the default. Remote input, clipboard, files, terminal,
system information, and unattended access stay independently permissioned. File
paths are confined to named roots and completed uploads are committed only after
SHA-256 verification. RemoteX does not invent cryptographic primitives.

## Current boundaries

RemoteX does not include mobile or macOS clients, audio, camera/printing, account
management, or a complete ICE/STUN/TURN NAT traversal stack. Self-hosters remain
responsible for host patching, TLS keys, backups, network policy, and public-edge
rate limiting.

Report vulnerabilities through a private GitHub security advisory as described
in [SECURITY.md](SECURITY.md).
