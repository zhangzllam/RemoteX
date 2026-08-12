# RemoteX Roadmap

Work proceeds milestone by milestone. A milestone is complete only after its
design, implementation, tests, documentation, formatting, linting, and existing
tests all pass.

## M0 — Foundation (completed)

- Cargo workspace and low-coupling crate boundaries
- versioned protocol domain types
- transport, crypto, capture, input, and file-transfer abstractions
- executable composition-root placeholders
- unit tests and architecture documentation

No operational remote-control functionality is included.

## M1 — Relay path (completed)

- Controller → relay → agent mock data path
- one-time session/role token verification
- peer pairing, binary forwarding, timeouts, and maximum frame size
- structured logging
- end-to-end integration test with mock peers

## M2 — Windows capture demo (completed)

- DXGI monitor enumeration and capture behind `ScreenCapture`
- cursor handling and display-mode recovery
- capture 100 frames and save one PNG in a standalone demo

## M3 — Remote screen (completed)

- DXGI capture frames with cursor composition
- software resize to no more than 1280×720
- independently decodable JPEG frames at a configurable 10–15 FPS target
- relay-only Agent → Controller video flow
- Tauri 2 + React remote display

## M4–M5 — Interactive input (next)

- **M4:** mouse movement, buttons, and wheel through Windows `SendInput`
- **M5:** keyboard down/up and modifiers through Windows `SendInput`

## M6 — File transfer

- directory browsing, upload, and download
- 4 MiB chunks, SHA-256 verification, progress, pause, cancel, and resume
- bounded memory usage for large files

## M7–M8 — Control and authorization

- **M7:** device registration, identity proof, heartbeat/presence, session
  creation, and scoped one-time tokens
- **M8:** visible accept/reject UI, opt-in unattended mode, independent
  permissions, source-device details, and session audit logs

## M9 — Video optimization

- tune JPEG/WebP first
- then evaluate H.264, hardware encoders, adaptive bitrate/FPS, and dirty rects

## M10 — Direct connectivity

- endpoint discovery, NAT traversal, and UDP hole punching
- attempt direct connection and always retain relay fallback

Audio, mobile clients, macOS, Linux desktop, remote camera/printing, and
multi-user collaboration remain outside the MVP. Linux terminal and file
management are future agent capabilities after the Windows MVP is stable.
