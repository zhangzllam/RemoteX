# Changelog

## 1.2.0 — 2026-08-13

### Performance and polish

- Unified incoming and outgoing server configuration with conservative v1.1
  migration and an explicit conflict choice.
- Added resumable first-run server verification and a settings entry for
  running setup again, without requiring device registration during setup.
- Controller-only use no longer presents local-device registration as a
  prerequisite; incoming Remote Access remains an explicit optional setting.
- Replaced the default RGBA base64 event path with a Tauri binary Channel,
  latest-frame scheduling, and received/rendered/dropped diagnostics.
- Split typed API, setup, video, diagnostics, tokens, layout, components, and
  session concerns out of the former single frontend module and CSS cascade.
- Added categorized connection feedback and an Apple-inspired graphite desktop
  hierarchy while retaining Windows behavior and explicit permission controls.
- Added an immediately applied, persisted interface-language setting for
  English and Simplified Chinese, with system-language detection on first use.
- Added signed-tag validation, version/artifact gates, optional Authenticode
  signing, performance guidance, and v1.2 release checks.

## 1.0.0 — 2026-08-12

First complete V1 release.

### Added

- Windows remote desktop, input, clipboard, and resumable file transfer.
- Linux remote terminal, rooted file transfer, and system information.
- Explicit local session authorization and opt-in unattended access.
- Adaptive H.264/JPEG video and authenticated direct-connect fallback.
- Self-hosted Control, Relay, PostgreSQL, Caddy, and production runbooks.
- Current-user Windows NSIS package with bundled Agent, tray, settings,
  autostart control, and DPAPI-protected local secrets.

### Security and reliability

- End-to-end XChaCha20-Poly1305 encrypted session payloads and authenticated
  QUIC/TLS transport.
- Bounded protocol messages, API bodies, connection/session queues, terminal
  sessions, file-transfer concurrency, and request duration.
- Signed device proofs with nonce replay protection and short-lived one-time
  role credentials.
- Malformed/truncated/corrupted wire-input tests and disconnect cleanup tests.
- RustSec and production frontend dependency audits included in CI.

### Known boundaries

- The downloadable Windows installer is not Authenticode-signed unless the
  release builder is configured with a trusted certificate.
- Direct connection uses configured candidates and is not a full ICE/STUN/TURN
  NAT traversal implementation.
- No mobile/macOS client, audio, account service, or hosted RemoteX cloud is
  included in V1.
