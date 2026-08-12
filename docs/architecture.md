# RemoteX Architecture

RemoteX is a self-hosted remote administration system. Milestones 0–7 establish
the module boundaries and a working, relay-only Windows remote-video, mouse,
keyboard control, plain-text clipboard, and safe remote-file paths.

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

| Crate | Responsibility | Still excluded through M7 |
| --- | --- | --- |
| `remotex-protocol` | Versioned wire-level domain types and serialization | Networking and platform code |
| `remotex-transport` | Transport-neutral async connection traits, framing, and QUIC stream adapter | Capture and protocol interpretation |
| `remotex-crypto` | XChaCha20-Poly1305 session encryption and identity abstractions | Custom cryptography and key persistence |
| `remotex-capture` | Platform-neutral capture model and Windows DXGI implementation | Networking and video encoding |
| `remotex-clipboard` | Permissioned text revisions, size limits, loop/conflict prevention, and Windows clipboard adapter | Images, HTML, and files |
| `remotex-input` | Permission enforcement, injected-state tracking, coordinate mapping, and Windows `SendInput` mouse/keyboard adapter | Local input capture |
| `remotex-file-transfer` | Virtual-root resolution, directory metadata, async chunk I/O, resume state, and SHA-256 verification | Networking and destructive deletion |
| `remotex-video` | 720p software image scaling, JPEG encoding, and JPEG/WebP decoding | Capture and transport |
| `remotex-agent` | Windows capabilities, persistent Ed25519 identity, heartbeat, and managed Session composition root | Local approval UI |
| `remotex-desktop` | Tauri/React controller with managed Session creation and manual development fallback | Installer packaging |
| `remotex-control` | Axum control API, PostgreSQL repository, presence, and credential issuance | User accounts |
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
        +--> capture / input / clipboard / file-transfer (where relevant)

transport, crypto, capture, input, clipboard, file-transfer --> protocol
    (only if shared identifiers or protocol data are required)
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
input. M6 and M7 use one monotonically increasing envelope/nonce sequence per
direction across video, input, clipboard, and files, preventing nonce reuse when
a new logical channel is added. The temporary development key is provisioned out of
band until the M8/M9 control plane distributes session keys. No home-grown
cryptographic algorithm is used.

## M8 control plane

On first managed startup the Agent creates an Ed25519 identity key locally and
registers only its public key. Registration is idempotent per public key and the
Control Server assigns a nine-digit `DeviceId`. Heartbeats and Agent Session
claims are domain-separated, timestamped, nonce-bearing signatures; the server
checks clock skew and atomically advances the stored nonce to reject replay.

The Desktop asks the HTTPS-friendly API for a Session by Device ID. PostgreSQL
stores the Session metadata and SHA-256 hashes of the two independently random,
role-bound Relay tokens. It never stores a raw Relay token. The response returns
the Controller credential once. The pending Agent credential and shared E2EE key
are encrypted at rest with the configured 256-bit Control Server master key and
are released exactly once to the signed Agent claim. The Relay validates and
consumes the applicable hash in a PostgreSQL row-locking transaction. Credentials
expire after five minutes by default.

```text
Agent --register/signed heartbeat--> Control API --> PostgreSQL
Desktop --create Session-----------> Control API --> PostgreSQL
Agent --signed claim---------------> Control API
Desktop + Agent --one-time credentials--> Relay --> opaque E2EE data
```

The Control Server derives `online` from `last_seen_ms`; no background flag can
leave a stale device permanently online. Manual Relay Session configuration is
retained only as a local development fallback.

## M9 authorization boundary

Managed Sessions begin in `pending` state. A signed Agent poll returns only a
sanitized incoming request, never the Agent Relay credential. By default the
Windows Agent displays a foreground native consent dialog naming the Controller
and each requested capability. Rejecting makes the Session terminal; accepting
atomically records the final permission subset before the wrapped Agent token is
released.

Unattended access is disabled by default. Enabling it requires both
`REMOTEX_UNATTENDED_ACCESS=true` and a 12–128 byte local secret. The Controller
must present the same secret; it is transported only over the Control API,
encrypted at rest with the Control Server master key, delivered only to the
signed Agent poll, and compared through fixed-length hashes. A missing or wrong
secret falls back to the visible local prompt rather than granting access.

The Agent enforces the final permissions at the subsystem boundary: screen
capture, input execution, clipboard, upload, and download are independent. An
active Session prints a prominent local banner and Ctrl+C disconnects it. The
Control Server audit trail contains request, accept/reject, start/end, Controller
name, permissions, connection type, result, timestamps, and aggregate bytes; it
never contains tokens, private keys, clipboard text, paths, input, or file data.

## Implemented data paths (M9)

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

```text
Windows UTF-16 clipboard ↔ bounded UTF-8 text ↔ ClipboardMessage(origin, revision)
    ↔ XChaCha20-Poly1305 ↔ QUIC/TLS ↔ Relay (ciphertext only) ↔ peer clipboard
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

M6 clipboard synchronization is disabled independently through
`REMOTEX_ALLOW_CLIPBOARD=false` by default and a Controller checkbox. Each side
polls plain text every 500 ms. A 1 MiB UTF-8 cap is enforced before sending and
again before applying. Each origin owns monotonically increasing revisions;
applied remote text is recorded as local state so it is not echoed. A Lamport-
style total order resolves simultaneous initial values deterministically and
both peers converge. Images, HTML, file clipboard formats, and clipboard content
logging are excluded.

## M11 direct data path

The Control Server stores the bounded candidate set from signed Agent
heartbeats and returns it with Controller credentials. Relay remains the
authorization rendezvous. After both authorized roles pair, they attempt a
three-second direct upgrade, preferring LAN over a configured public UDP mapping.

```text
Control authorization -> Relay pairing -> authenticated direct QUIC
                                      \-> Relay fallback
```

Direct TLS uses a CA-trusted Agent certificate and an application handshake
uses HMAC-SHA256 proofs derived from the E2EE Session key, role-separated and
bound to fresh nonces and the Session ID. Direct and Relay paths expose the same
bounded `Connection` interface, so video, input, clipboard, and files cannot
bypass their existing permission or encryption boundaries.

## M12 Linux server Agent

The Linux Agent shares the M8–M11 enrollment, authorization, E2EE, Relay, and
direct-transport path. It deliberately exposes server-oriented capabilities
instead of a Linux desktop: bounded PTYs, the existing M7 rooted file service,
and a bounded read-only system snapshot. Terminal and system-information grants
are explicit fields in the Agent-enforced Session permissions.

Each authorized PTY is owned by the Session and runs as the unprivileged Agent
OS account. A bounded reader queue prevents a fast process from accumulating
unlimited output. Explicit close and Session teardown kill PTY children. Default
foreground mode shows an accept/reject prompt and active-session banner; systemd
operation is possible only after an administrator installs the provided unit,
and unattended authorization remains separately opt-in.

## M10 video pipeline

After the E2EE path is ready, the Controller advertises H.264, JPEG, and WebP.
The Agent negotiates H.264 when available and otherwise retains its JPEG
fallback. Capture, codec, and transport remain separate abstractions. OpenH264
provides the initial software encoder/decoder; later hardware encoders can be
introduced behind the same traits.

```text
DXGI BGRA -> adaptive scale -> H.264 -> E2EE envelope -> transport
          -> persistent decoder -> RGBA canvas + telemetry overlay
```

The Controller returns at most one encrypted feedback report per second. Three
poor samples lower resolution/FPS/bitrate and six good samples raise them. Full
frames remain bounded to 3840×2160 and encoded frames to 8 MiB. JPEG remains the
pre-negotiation and encoder-failure path.

## M7 file path and transfer model

M7 exposes only configured Agent roots under virtual names such as `/Documents`.
Every existing path is canonicalized and checked against its root; new paths
require an already-canonical parent inside that root. Parent traversal,
backslashes, unknown roots, symlink/junction escapes, root mutation, overwrite,
recursive deletion, and non-file special entries are rejected. Upload and
download have separate Agent flags and Controller checkboxes.

```text
Controller local file -> 4 MiB chunk -> SHA-256 -> FileTransferMessage
    -> XChaCha20-Poly1305 -> bounded QUIC/Relay path -> Agent file worker
    -> virtual-root validation -> hidden partial file -> verified final file
```

File I/O runs in a separate async worker behind bounded command/response queues.
Exactly one chunk is in flight per transfer: the next chunk is read only after
`Accept`/`ChunkAck`, providing backpressure while video and input retain their
own scheduling branches. Interrupted partial files use the transfer ID and can
resume at their persisted length. Cancellation removes a receiving partial;
disconnect closes handles but preserves partials for explicit same-ID resume.
Each chunk and the completed file use SHA-256, and a destination becomes visible
only after final verification.

## M13 self-hosted deployment boundary

Production Control and Relay binaries run as non-root, read-only containers.
PostgreSQL lives only on an internal network, Control is reachable only through
Caddy HTTPS, and Relay publishes only its bounded QUIC UDP port. Runtime secrets
are mounted as read-only files rather than committed or embedded in images.
Control readiness includes a live database query; Relay has a local container
health endpoint. Both services emit structured JSON without payload contents.

## Error handling and observability

Library crates expose typed errors with `thiserror`. Executables may add context
with `anyhow`. Expected failures are returned rather than handled with `unwrap`.
Later network services will emit structured `tracing` events containing safe
identifiers, never secrets or payload contents.

## M0–M7 acceptance criteria

- the Cargo workspace builds on stable Rust;
- shared protocol values serialize deterministically and round-trip in tests;
- transport and platform contracts can be mocked without operating-system APIs;
- file-transfer validates virtual roots, streams bounded chunks, resumes partials,
  and verifies SHA-256 before finalizing files;
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
- Controller and Agent synchronize empty and multilingual UTF-8 text in both
  directions without echo loops;
- clipboard text is capped at 1 MiB and independently permission-gated;
- all encrypted logical channels share one sequence per direction, preventing
  nonce reuse;
- directory and transfer paths cannot escape explicitly configured roots;
- uploads and downloads are independently permission-gated, chunked, resumable,
  cancellable, progress-reporting, and SHA-256 verified;
- file work uses bounded queues and acknowledgement-driven backpressure;
- the React production frontend and Tauri backend build successfully.
