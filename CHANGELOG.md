# Changelog

## 1.3.1 – 2026-09-10

- Pin release Rust to the locally validated 1.97.1 toolchain. Includes all 1.3.0 changes; the 1.3.0 CI run did not publish binaries.

## 1.3.0 – 2026-09-10

### Connectivity

- Added a deployment-scoped, certificate-verified IP Control endpoint on TCP
  7443, with automatic migration of the former built-in domain. Desktop and
  both Agents share the same trust/proxy policy; custom servers are preserved.
- Resolve the built-in Relay directly by IP while retaining TLS hostname
  validation, avoiding dependence on VPN Fake-IP DNS for this deployment.
- Added standards-compliant STUN binding discovery on the persistent Direct
  QUIC socket, short-lived server-reflexive candidates, expiry validation, and
  deterministic Host / discovered / configured-public ranking.
- Made Relay usable immediately while authenticated Direct checks run in the
  background, with an ordered four-step switch barrier and automatic Relay
  recovery after transient Direct or Relay loss.
- Added bounded reconnect backoff and separate, role-bound, expiring recovery
  credentials without making initial one-time credentials reusable.

### Performance and experience

- Refined the bilingual light/dark Home screen around incoming and outgoing
  connections, with visible device credentials and compact laptop layouts.
- Generate a protected connection password on first remote-access activation;
  support repeated random regeneration, custom passwords, explicit reveal/copy,
  and approval-only access. Prevent password edits during an incoming session.
- Hide password displays on window blur, discard delayed reveal responses after
  focus loss, prevent self-connections, and clear passwords on target changes.
- Add recent-device search, removal, and undo.

- Added real QUIC RTT/loss, bitrate, capture/encode/decode/render timing,
  receive/render/drop, resolution, codec, and estimated queue diagnostics;
  unavailable measurements now display an em dash instead of fake zeroes.
- Added a compact localized P2P Direct / Relay quality control that opens the
  diagnostics overlay, plus Auto, Quality, Balanced, and Low bandwidth video
  profiles with render/drop-aware adaptation hysteresis.

### Reliability and security

- Fixed saving Remote Access settings on Windows when Start with Windows was
  already disabled: an absent startup entry no longer blocks Agent startup.
- Added an explicit path-selection model, credential recovery tests, bounded
  500-of-1000 idle-session capacity testing, and retained exact E2EE sequence
  validation across transport recovery.
- Removed complete peer addresses from normal Relay logs and added local-only
  aggregate counters for sessions, authentication failures, forwarded bytes,
  timeouts, slow consumers, and Direct upgrades.

## 1.2.2 — 2026-08-13

- Fixed the GitHub Actions release workflow YAML so signed release tags can
  be fetched as annotated objects, verified, built, and published.

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
