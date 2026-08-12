# RemoteX

RemoteX is a greenfield, self-hosted remote administration system inspired by
the architecture of tools such as RustDesk, ToDesk, and MeshCentral. It is
designed for visible, authorized access to a user's own Windows PCs and Linux
servers.

The repository currently implements **Milestones 0–4**:

- a modular Rust workspace and versioned protocol model;
- an authenticated QUIC relay with one-time, role-bound credentials;
- explicit peer readiness, heartbeat, disconnect cleanup, and bounded forwarding;
- XChaCha20-Poly1305 end-to-end frame encryption, leaving the relay blind;
- Windows DXGI Desktop Duplication with cursor composition and mode recovery;
- 720p JPEG software encoding at a configurable 10–15 FPS target;
- a Tauri 2 + React controller that displays relayed desktop frames;
- permission-gated, end-to-end encrypted Windows mouse movement, buttons, and
  wheel input through `SendInput`.

Keyboard control, clipboard synchronization, file I/O, the control-server API,
persistent device registration, and P2P are intentionally not implemented yet.

## Workspace

```text
crates/   shared protocol, capability, capture, and video modules
apps/     agent and desktop composition roots
servers/  control and relay composition roots
docs/     architecture, protocol, and milestone specifications
```

## Development

Rust stable is required.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

The desktop frontend also requires Node.js and pnpm:

```bash
pnpm --dir apps/desktop/ui install --frozen-lockfile
pnpm --dir apps/desktop/ui build
```

See [Architecture](docs/architecture.md), [Protocol](docs/protocol.md), and
[Roadmap](docs/roadmap.md) for design boundaries and planned work. See
[Relay M1](docs/relay.md) for the mock transport workflow and
[Running M4](docs/running-m4.md) for the current development-only relay, agent,
and controller workflow.

## Security posture

RemoteX is intended to be a normal, visible administration tool. Interactive
approval is the default, unattended access will be opt-in, capabilities will be
permissioned independently, and session activity will be visible and audited.
M3/M4 video and input envelopes use XChaCha20-Poly1305 authenticated encryption
above QUIC/TLS, with direction-separated nonces derived from the Session ID and
sequence. The relay forwards ciphertext and cannot decode desktop frames or
mouse events. M4 input is disabled unless the Agent user explicitly enables the
local permission. No custom cryptographic algorithms are introduced.
