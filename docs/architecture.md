# RemoteX Architecture

RemoteX is a self-hosted remote administration system. Milestones 0–5 establish
the module boundaries and a working, relay-only Windows remote-video, mouse,
and keyboard control path.

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

| Crate | Responsibility | Still excluded through M5 |
| --- | --- | --- |
| `remotex-protocol` | Versioned wire-level domain types and serialization | Networking and platform code |
| `remotex-transport` | Transport-neutral async connection traits, framing, and QUIC stream adapter | Capture and protocol interpretation |
| `remotex-crypto` | XChaCha20-Poly1305 session encryption and identity abstractions | Custom cryptography and key persistence |
| `remotex-capture` | Platform-neutral capture model and Windows DXGI implementation | Networking and video encoding |
| `remotex-input` | Permission enforcement, injected-state tracking, coordinate mapping, and Windows `SendInput` mouse/keyboard adapter | Clipboard and local input capture |
| `remotex-file-transfer` | Chunk planning and resumable-transfer state model | Filesystem I/O and networking |
| `remotex-video` | 720p software image scaling, JPEG encoding, and JPEG/WebP decoding | Capture and transport |
| `remotex-agent` | Windows capture/video send and authorized mouse/keyboard receive/execution composition root | Device enrollment and clipboard |
| `remotex-desktop` | Tauri/React video display and focused mouse/keyboard sender | Clipboard and file transfer |
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

## M1 relay networking

Both peers initiate an outbound Quinn connection and open one bidirectional
QUIC stream. The first framed value is `ClientHello`. After authentication, the
Relay registers the peer in an in-memory Session entry and returns
`WaitingForPeer` or `PeerReady`. Application data is carried inside
`RelayClientMessage::Payload` and `RelayServerMessage::Payload`; the Relay only
examines this outer variant and never decodes the contained bytes.

Each connected peer owns one reader loop and one writer task. The writer is fed
by a bounded Tokio channel (32 entries by default), so a blocked destination
does not block reads or the opposite direction indefinitely. A full queue closes
the Session instead of growing memory without bound. The Session registry uses
one `Mutex<HashMap<...>>`; locks protect only short lookup/update operations and
are always released before network I/O or channel waits. `JoinSet` gives the
server ownership of every connection task during shutdown.

Session state is derived from two optional peer slots:

```text
Controller only -> WaitingForAgent
Agent only      -> WaitingForController
Both present    -> Ready
Peer removed    -> Closed and registry entry removed
```

The Relay sends application-level heartbeats on a configurable interval. Any
valid inbound Relay message refreshes liveness; an inactive peer is closed after
`peer_timeout`. Disconnect, timeout, malformed input, an oversized frame, or a
slow consumer removes the whole Session and signals the remaining peer.

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
ciphertext. M4 uses the opposite cryptographic direction for Controller-to-Agent
input and enforces an independent monotonically increasing input sequence. The
temporary development key is provisioned out of band until the M8/M9 control
plane distributes session keys. No home-grown cryptographic algorithm is used.

## Implemented data paths (M5)

```text
Windows DXGI → compact BGRA → 1280×720 resize → JPEG → MessageEnvelope
             → XChaCha20-Poly1305 → QUIC/TLS → Relay (ciphertext only) → QUIC/TLS
             → Tauri backend → data URL → React remote display
```

```text
Focused React pointer/wheel/keyboard event → platform-neutral InputEvent
    → XChaCha20-Poly1305 → QUIC/TLS → Relay (ciphertext only)
    → Agent sequence/authentication check → permission gate → InputState
    → Windows SendInput
```

The default video rate is 12 FPS and can be configured from 1–30 FPS. The M3
target remains stability at 10–15 FPS rather than high frame rate.

The React client accounts for `object-fit: contain` letterboxing and sends
coordinates in the full 0–65535 protocol range. The Agent maps them first to the
selected monitor and then to the Windows virtual-desktop absolute range. The
protocol carries an optional validated display ID for later multi-monitor UI.
M4/M5 default `control_input` to false; the local Agent user must set
`REMOTEX_ALLOW_INPUT=true`. Duplicate button transitions are ignored and all
buttons and keys injected by a Session are released on normal shutdown, error,
network disconnect, focus loss, or controller drop. Keyboard events originate
only from the explicitly focused remote surface; the Agent never captures local
keystrokes.

## Error handling and observability

Library crates expose typed errors with `thiserror`. Executables may add context
with `anyhow`. Expected failures are returned rather than handled with `unwrap`.
Later network services will emit structured `tracing` events containing safe
identifiers, never secrets or payload contents.

## M0–M5 acceptance criteria

- the Cargo workspace builds on stable Rust;
- shared protocol values serialize deterministically and round-trip in tests;
- transport and platform contracts can be mocked without operating-system APIs;
- file-transfer state validates chunk boundaries without reading files;
- `cargo fmt`, `cargo clippy --workspace --all-targets --all-features`, and
  `cargo test --workspace --all-features` pass.
- mock controller and agent exchange frames through a real local QUIC relay;
- both peers receive `PeerReady`, answer heartbeats, and observe Session closure;
- the Windows demo captures 100 frames and writes a valid PNG;
- a 1080p BGRA test frame scales to 720p, crosses the relay, and decodes;
- relayed video remains authenticated end-to-end ciphertext until the controller;
- relayed input remains authenticated end-to-end ciphertext until the Agent;
- normalized coordinates map to the selected Windows monitor;
- unauthorized input is rejected before Windows execution;
- pressed mouse buttons are released during Session cleanup;
- common physical keys, navigation keys, F1–F12, and left/right modifiers map
  to Windows virtual keys without exposing Windows codes in the protocol;
- duplicate key transitions are ignored and pressed keys are released during
  focus and Session cleanup;
- the React production frontend and Tauri backend build successfully.
