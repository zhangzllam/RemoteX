# RemoteX Roadmap

Work proceeds one milestone at a time. A milestone is complete only after its
design, implementation, tests, documentation, formatting, linting, and existing
tests all pass.

## Completed

- **M0 — Foundation:** workspace boundaries, protocol, capability contracts,
  and tests.
- **M1 — Relay transport:** authenticated QUIC pairing, role-bound one-time
  credentials, heartbeat, bounded opaque forwarding, and cleanup.
- **M2 — Windows capture:** DXGI monitor enumeration, cursor composition, mode
  recovery, and capture demo.
- **M3 — Remote screen:** 720p JPEG stream, end-to-end encryption, relay path,
  and Tauri/React display.
- **M4 — Remote mouse:** normalized movement, left/right/middle buttons, wheel,
  future display identifier, explicit local input permission, Windows
  `SendInput`, input-state cleanup, and encrypted relay integration tests.

## Next

- **M5 — Remote keyboard:** platform-neutral keys and modifiers, Windows
  injection, pressed-key tracking, and release on disconnect.
- **M6 — Clipboard:** bounded bidirectional UTF-8 text with independent
  permission and revision-based loop prevention.
- **M7 — Files:** safe rooted directory browsing plus chunked, resumable,
  checksummed upload and download without blocking interactive traffic.
- **M8 — Control server:** PostgreSQL-backed device registration, identity,
  presence, Session creation, and short-lived Relay credentials.
- **M9 — Authorization:** visible accept/reject UI, per-capability permissions,
  opt-in unattended access, active-Session controls, and audit records.
- **M10 — Video optimization:** codec abstraction and negotiation, H.264 with
  JPEG/WebP fallback, adaptive quality, and latency telemetry.
- **M11 — Direct connectivity:** authenticated LAN/P2P attempts, bounded NAT
  traversal, and reliable Relay fallback.
- **M12 — Linux server Agent:** authorized PTY terminal, M7 Files reuse, system
  information, and documented visible service operation.
- **M13 — Self-hosting:** Docker images, Compose stack, PostgreSQL isolation,
  reverse-proxy TLS, health checks, and deployment documentation.
- **M14 — Windows usability:** Tauri installer, Agent packaging, opt-in startup,
  settings, visible tray state, and secure update preparation.
- **M15 — Final review:** systematic security/resource review, malformed-input
  coverage, lifecycle cleanup tests, end-to-end validation, and V1 checklists.

Mobile clients, macOS, audio, remote camera/printing, and unrelated V2 features
remain outside the V1 plan.
