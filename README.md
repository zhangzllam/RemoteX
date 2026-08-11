# RemoteX

RemoteX is a greenfield, self-hosted remote administration system inspired by
the architecture of tools such as RustDesk, ToDesk, and MeshCentral. It is
designed for visible, authorized access to a user's own Windows PCs and Linux
servers.

This repository currently contains **Milestone 0 only**: the Rust workspace,
versioned protocol model, platform/transport abstractions, pure file-transfer
state validation, tests, and architecture documentation. It does not yet
capture screens, inject input, read files, forward relay traffic, or provide a
usable remote-control session.

## Workspace

```text
crates/   shared protocol and capability abstractions
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

See [Architecture](docs/architecture.md), [Protocol](docs/protocol.md), and
[Roadmap](docs/roadmap.md) for design boundaries and planned work.

## Security posture

RemoteX is intended to be a normal, visible administration tool. Interactive
approval is the default, unattended access will be opt-in, capabilities will be
permissioned independently, and session activity will be visible and audited.
No custom cryptographic algorithms will be introduced.

