# RemoteX

RemoteX is a self-hosted remote-control system for visibly and explicitly
accessing your own Windows PCs and Linux servers. The project includes a Windows
desktop application, a headless Linux component, an HTTPS Control Server, and
an end-to-end encrypted QUIC Relay.

## 在其他电脑上下载安装

打开 [最新版下载页](https://github.com/zhangzllam/RemoteX/releases/latest)，
在 **Assets** 中下载 `RemoteX_1.3.1_x64-setup.exe`，适用于 Windows x64。
无需下载 Source code，也无需复制开发项目。

两台电脑都安装 RemoteX。在被控电脑开启「允许远程访问」，等待设备 ID
出现，并查看连接密码；在控制电脑输入该 ID 和密码即可发起连接。
需要远程操作键鼠时，在被控电脑的「设置 → 权限」开启键盘和鼠标权限。
可在安全设置中改为每次手动确认。升级前请从托盘退出旧版 RemoteX。

v1.3.1 已包含本项目的默认服务器配置。完整跨网络会话仍需按
[测试矩阵](docs/v1.3-network-test-matrix.md) 实测，服务器就绪不代表所有网络
场景都已通过验收。

## Capabilities

- Windows desktop capture with cursor composition, monitor selection, adaptive
  H.264/JPEG video, mouse, keyboard, clipboard, and resumable file transfer.
- Connection passwords generated when first enabling remote access, optional
  foreground accept/reject, independent permissions, and visible local disconnect.
- Linux PTY terminals, safe rooted file transfer, and read-only system metrics.
- Ed25519 device enrollment, nine-digit Device IDs, signed presence, one-time
  role-bound credentials, and short-lived session material.
- Standards-based public UDP mapping discovery plus mutually authenticated
  direct QUIC checks, with fast Relay readiness and automatic recovery.
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
and the [v1.2 release checklist](docs/v1.2-release-checklist.md). The v1.3
network scenarios and measurement rules are in the
[network test matrix](docs/v1.3-network-test-matrix.md) and
[connection performance guide](docs/connection-performance.md).

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
management, or a complete ICE/TURN stack. Its conservative STUN-assisted checks
do not guarantee Direct connectivity through symmetric NAT, CGNAT, UDP blocking,
or hostile firewalls; Relay remains the encrypted fallback. Self-hosters remain
responsible for host patching, TLS keys, backups, network policy, and public-edge
rate limiting.

Report vulnerabilities through a private GitHub security advisory as described
in [SECURITY.md](SECURITY.md).
