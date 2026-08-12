# RemoteX Architecture

RemoteX is a self-hosted remote administration system. Milestones 0–3 establish
the module boundaries and a working, relay-only Windows remote-video path.

## System boundaries

The target system has four deployable roles:

- **Controller (`apps/desktop`)** presents the desktop UI and originates control
  requests.
- **Agent (`apps/agent`)** runs visibly on an explicitly enrolled device and
  asks the local user to approve incoming sessions by default.
- **Control server (`servers/control`)** owns device registration, presence,
  session authorization, permissions, and audit records.
- **Relay server (`servers/relay`)** pairs authenticated session peers and
  forwards opaque encrypted frames without interpreting their contents.

The control plane and data plane stay separate. HTTPS/WebSocket control-plane
traffic creates and authorizes a session. Remote desktop, input, clipboard, and
file-transfer data later travel through QUIC, initially via the relay.

## Workspace modules

| Crate | Responsibility | Still excluded through M3 |
| --- | --- | --- |
| `remotex-protocol` | Versioned wire-level domain types and serialization | Networking and platform code |
| `remotex-transport` | Transport-neutral async connection traits, framing, and QUIC stream adapter | Capture and protocol interpretation |
| `remotex-crypto` | XChaCha20-Poly1305 session encryption and identity abstractions | Custom cryptography and key persistence |
| `remotex-capture` | Platform-neutral capture model and Windows DXGI implementation | Networking and video encoding |
| `remotex-input` | Platform-neutral input execution contract | Windows `SendInput` |
| `remotex-file-transfer` | Chunk planning and resumable-transfer state model | Filesystem I/O and networking |
| `remotex-video` | 720p software image scaling, JPEG encoding, and JPEG/WebP decoding | Capture and transport |
| `remotex-agent` | Windows capture → encode → QUIC composition root | Device enrollment and remote input |
| `remotex-desktop` | Tauri/React controller and QUIC video receiver | Remote input and file transfer |
| `remotex-control` | Control-server composition root | HTTP APIs and database |
| `remotex-relay` | QUIC authentication, pairing, and opaque frame forwarding | Payload parsing and storage |

Platform-independent crates must not depend on application crates. Applications
compose capabilities through traits and may select concrete implementations at
runtime. The protocol crate is the only source of shared wire types.

## Dependency direction

```text
apps/*, servers/*
        |
        +--> protocol
        +--> transport
        +--> crypto
        +--> capture / input / file-transfer (where relevant)

transport, crypto, capture, input, file-transfer --> protocol (only if shared
identifiers or protocol data are required)
```

No library crate depends on an executable crate. Capture and input crates do not
contain networking. Transport does not contain platform APIs.

## Session and security model

A session is identified by an opaque `SessionId`. A one-time, expiring token is
bound to the session and to exactly one role (`Controller` or `Agent`). The relay
will eventually authenticate both peers and only forward encrypted frames.

Remote access must remain visible and consensual:

- an agent is explicitly installed and enrolled;
- interactive approval is the default;
- unattended access is opt-in;
- desktop, input, clipboard, and file permissions are independent;
- an active connection is visibly indicated and audited;
- the relay cannot read end-to-end encrypted payloads.

The M1 transport uses QUIC/TLS through Quinn and rustls. Session credentials are
256-bit opaque values compared in constant time, bound to one Session/Role pair,
and consumed once before expiry. M3 adds XChaCha20-Poly1305 authenticated
encryption above relay TLS. Its direction-separated nonce is derived from the
Session ID and monotonically increasing sequence, so the relay forwards only
ciphertext. The temporary development key is provisioned out of band until the
M7/M8 control plane distributes session keys. No home-grown cryptographic
algorithm is used.

## Implemented data path (M3)

```text
Windows DXGI → compact BGRA → 1280×720 resize → JPEG → MessageEnvelope
             → XChaCha20-Poly1305 → QUIC/TLS → Relay (ciphertext only) → QUIC/TLS
             → Tauri backend → data URL → React remote display
```

The default video rate is 12 FPS and can be configured from 1–30 FPS. The M3
target remains stability at 10–15 FPS rather than high frame rate.

## Error handling and observability

Library crates expose typed errors with `thiserror`. Executables may add context
with `anyhow`. Expected failures are returned rather than handled with `unwrap`.
Later network services will emit structured `tracing` events containing safe
identifiers, never secrets or payload contents.

## M0–M3 acceptance criteria

- the Cargo workspace builds on stable Rust;
- shared protocol values serialize deterministically and round-trip in tests;
- transport and platform contracts can be mocked without operating-system APIs;
- file-transfer state validates chunk boundaries without reading files;
- `cargo fmt`, `cargo clippy --workspace --all-targets --all-features`, and
  `cargo test --workspace --all-features` pass.
- mock controller and agent exchange frames through a real local QUIC relay;
- the Windows demo captures 100 frames and writes a valid PNG;
- a 1080p BGRA test frame scales to 720p, crosses the relay, and decodes;
- relayed video remains authenticated end-to-end ciphertext until the controller;
- the React production frontend and Tauri backend build successfully.
