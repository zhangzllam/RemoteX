# RemoteX

RemoteX is a greenfield, self-hosted remote administration system inspired by
the architecture of tools such as RustDesk, ToDesk, and MeshCentral. It is
designed for visible, authorized access to a user's own Windows PCs and Linux
servers.

The repository currently implements **Milestones 0–6**:

- a modular Rust workspace and versioned protocol model;
- an authenticated QUIC relay with one-time, role-bound credentials;
- explicit peer readiness, heartbeat, disconnect cleanup, and bounded forwarding;
- XChaCha20-Poly1305 end-to-end frame encryption, leaving the relay blind;
- Windows DXGI Desktop Duplication with cursor composition and mode recovery;
- 720p JPEG software encoding at a configurable 10–15 FPS target;
- a Tauri 2 + React controller that displays relayed desktop frames;
- permission-gated, end-to-end encrypted Windows mouse movement, buttons, and
  wheel input through `SendInput`;
- focused remote keyboard input for common keys and modifiers, with pressed-key
  cleanup on focus loss, disconnect, error, and shutdown;
- opt-in, bidirectional UTF-8 text clipboard synchronization with a 1 MiB limit,
  revision/origin loop prevention, and a separate Agent permission.

File I/O, the control-server API, persistent device registration, and P2P are
intentionally not implemented yet.

## Workspace

```text
crates/   shared protocol, capability, and platform-adapter modules
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
[Running M6](docs/running-m6.md) for the current development-only relay, agent,
and controller workflow.

## Security posture

RemoteX is intended to be a normal, visible administration tool. Interactive
approval is the default, unattended access will be opt-in, capabilities will be
permissioned independently, and session activity will be visible and audited.
M3–M6 video, input, and clipboard envelopes use XChaCha20-Poly1305 authenticated
encryption above QUIC/TLS, with direction-separated nonces derived from the
Session ID and sequence. The relay forwards ciphertext and cannot decode desktop
frames, input events, or clipboard contents. Remote input and clipboard
permissions are independent and disabled unless the Agent user explicitly
enables them. No custom cryptographic algorithms are introduced.
