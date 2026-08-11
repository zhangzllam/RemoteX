# RemoteX Architecture

RemoteX is a self-hosted remote administration system. Milestone 0 establishes
the boundaries and shared vocabulary needed by later milestones; it does not
provide a working remote-control path.

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

| Crate | Responsibility | Explicitly excluded in M0 |
| --- | --- | --- |
| `remotex-protocol` | Versioned wire-level domain types and serialization | Networking and platform code |
| `remotex-transport` | Transport-neutral async connection traits | QUIC implementation and capture |
| `remotex-crypto` | Session cipher and identity abstractions | Custom cryptography and key persistence |
| `remotex-capture` | Platform-neutral screen capture model | DXGI and encoding |
| `remotex-input` | Platform-neutral input execution contract | Windows `SendInput` |
| `remotex-file-transfer` | Chunk planning and resumable-transfer state model | Filesystem I/O and networking |
| `remotex-agent` | Agent process composition root | Device enrollment and remote execution |
| `remotex-desktop` | Controller process composition root | Tauri/React UI and video display |
| `remotex-control` | Control-server composition root | HTTP APIs and database |
| `remotex-relay` | Relay-server composition root | Pairing and frame forwarding |

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

M0 defines cryptographic boundaries but intentionally selects no home-grown
algorithm. A later milestone must use reviewed libraries and standard protocols.

## Error handling and observability

Library crates expose typed errors with `thiserror`. Executables may add context
with `anyhow`. Expected failures are returned rather than handled with `unwrap`.
Later network services will emit structured `tracing` events containing safe
identifiers, never secrets or payload contents.

## M0 acceptance criteria

- the Cargo workspace builds on stable Rust;
- shared protocol values serialize deterministically and round-trip in tests;
- transport and platform contracts can be mocked without operating-system APIs;
- file-transfer state validates chunk boundaries without reading files;
- `cargo fmt`, `cargo clippy --workspace --all-targets --all-features`, and
  `cargo test --workspace --all-features` pass.

