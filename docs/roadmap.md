# RemoteX roadmap

RemoteX V1 is complete. Each milestone includes implementation, tests,
documentation, formatting, linting, and integration into the final release.

## Completed V1 milestones

- **M0 — Foundation:** workspace, protocol, capability contracts, and tests.
- **M1 — Relay:** authenticated QUIC pairing, bounded forwarding, and cleanup.
- **M2 — Windows capture:** DXGI monitors, cursor, and mode recovery.
- **M3 — Remote screen:** encrypted video through Relay and Tauri display.
- **M4 — Mouse:** normalized input, permission enforcement, and state cleanup.
- **M5 — Keyboard:** portable keys/modifiers, ordering, and release on cleanup.
- **M6 — Clipboard:** opt-in bounded UTF-8 sync with loop prevention.
- **M7 — Files:** rooted, resumable, checksummed upload and download.
- **M8 — Control:** enrolled devices, signed presence, PostgreSQL, sessions.
- **M9 — Authorization:** local consent, unattended opt-in, and audit records.
- **M10 — Video:** adaptive OpenH264 with negotiated JPEG fallback.
- **M11 — Direct:** authenticated candidate attempts and Relay fallback.
- **M12 — Linux Agent:** PTY, rooted files, system information, and systemd.
- **M13 — Self-hosting:** containers, PostgreSQL, Caddy, secrets, and runbooks.
- **M14 — Windows usability:** NSIS, Agent sidecar, settings, tray, and DPAPI.
- **M15 — Final review:** security/resource limits, malformed-input coverage,
  dependency audit, lifecycle review, release automation, and V1 checklists.

## Possible V2 work

Mobile and macOS clients, audio, camera/printing, account management, full
ICE/STUN/TURN traversal, signed auto-update infrastructure, and fleet policy are
outside V1 and require separate design and threat review.
