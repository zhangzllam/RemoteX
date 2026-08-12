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
- **M4 — Remote mouse:** normalized movement, buttons, wheel, future display ID,
  explicit local input permission, `SendInput`, and input-state cleanup.
- **M5 — Remote keyboard:** platform-neutral common keys and modifiers, focused
  Controller capture, `SendInput`, ordered combinations, duplicate suppression,
  and release on focus loss or Session cleanup.
- **M6 — Clipboard:** opt-in bidirectional UTF-8 text, 1 MiB limit, independent
  permission, loop prevention, simultaneous-change convergence, and tests.
- **M7 — Files:** safe rooted directory browsing, directory creation, chunked
  upload/download, bounded acknowledgement backpressure, cancellation, progress,
  same-ID resume, and SHA-256 completion verification.
- **M8 — Control server:** Ed25519 device enrollment, nine-digit Device IDs,
  signed heartbeat and claims, PostgreSQL presence and Session records,
  wrapped pending secrets, and short-lived one-time Relay credentials.
- **M9 — Authorization:** foreground local accept/reject, Agent-enforced
  per-capability grants, secret-gated opt-in unattended access, visible active
  state with local disconnect, and signed lifecycle audit records.

## Next

M10 is complete: negotiated OpenH264 software encoding, JPEG fallback,
persistent decoding, bounded adaptive quality, frame telemetry, and a Controller
debug overlay are implemented. The remaining milestones start at M11.

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
