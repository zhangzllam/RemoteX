# RemoteX V1 security and resource review

Review date: 2026-08-12. Scope: the V1 workspace, Windows and Linux Agents,
desktop controller, Control Server, Relay, packaging, and deployment assets.
This is an internal engineering review, not a third-party penetration test.

## Trust and authorization boundaries

- Device enrollment uses an Ed25519 identity. Heartbeats, claims, decisions,
  and lifecycle events carry signed proofs with clock and monotonic-nonce replay
  checks.
- A controller receives a short-lived one-time role credential. The Agent gets
  a distinct credential only after foreground acceptance or successful opt-in
  unattended-secret verification.
- The Agent intersects requested capabilities with locally enabled capabilities.
  View, input, clipboard, files, terminal, and system information are enforced
  independently at the execution point.
- Active sessions remain visible and locally disconnectable. Linux systemd and
  Windows autostart are opt-in.

## Cryptography and secrets

- QUIC uses TLS and role authentication. Session payloads also use
  XChaCha20-Poly1305 with direction-separated nonces derived from session and
  sequence state; the Relay sees ciphertext only.
- Server-held pending secrets are wrapped with the configured master key and
  expire with the session. Role tokens are stored as hashes.
- Windows unattended secrets and the Windows Agent Ed25519 private key are
  protected with current-user DPAPI. A legacy plaintext identity is migrated on
  first successful read. Secret-bearing settings types do not derive `Debug`.
- Deployment secrets are mounted from ignored, read-only files. Logs were
  scanned for token/key/secret values; only metadata-safe identifiers, state,
  and outcomes are emitted.
- The v1.2 unified server file contains endpoints and a CA certificate path,
  but no device private key, unattended secret, Session credential, or updater
  private key. Legacy configuration migration never copies secret fields into
  the WebView.

## Parser and resource limits

- The bincode wire decoder has an 8 MiB allocation/size budget, rejects trailing
  bytes, and is tested with truncated, malformed, corrupted, invalid UTF-8, and
  oversized input.
- Transport frames, video/clipboard/file chunks, Relay connections/sessions,
  channel queues, PTYs, and concurrent file transfers all have hard limits.
- The Control API accepts at most 64 KiB per request, times out requests after
  20 seconds, and permits at most 32 unexpired pending sessions per device. The
  PostgreSQL limit is serialized with a device-row lock.
- File transfer permits at most eight simultaneous transfers per session. File
  paths resolve only below explicit virtual roots; traversal and unknown roots
  are rejected. Interrupted partial uploads remain resumable, cancellation
  removes partial data, and final commit requires the expected SHA-256 digest.
- Input state is released on focus/session loss, Relay peer tasks terminate on
  disconnect/timeout, and PTY child processes are killed during cleanup.

## Dependency audit

`cargo audit` checked 633 locked Rust packages against 1,211 RustSec advisories.
The vulnerable `quick-xml` and `time` versions found during review were upgraded
to `quick-xml 0.41.0` and `time 0.3.47`. `RUSTSEC-2023-0071` is explicitly
ignored because `rsa` exists only in the unused `sqlx-mysql` lockfile branch;
RemoteX disables SQLx defaults, enables PostgreSQL only, and `cargo tree -i rsa`
shows it is absent from shipped targets. The advisory has no fixed release.

RustSec also reports maintenance warnings for bincode, rustls-pemfile, and
target-specific transitive GTK/UNIC packages. They are not known exploitable
vulnerabilities in the shipped Windows/server paths. The bincode boundary is
size-limited and fuzz-style malformed-input tested; replacing these maintained
interfaces is tracked as V2 dependency work. `pnpm audit --prod` reports no
known production frontend vulnerabilities.

## Deployment and residual risk

- Put public Control endpoints behind Caddy plus external edge rate limiting or
  a trusted-network policy. V1 has Device IDs and device/session authorization,
  but no end-user account or fleet identity provider.
- Direct connectivity covers authenticated configured LAN/public candidates,
  not full ICE/STUN/TURN NAT traversal. Relay fallback remains required.
- The standard local Windows installer is current-user scoped but unsigned when
  no trusted Authenticode certificate is supplied. Verify the published SHA-256
  file before installation.
- Actual Internet routing, certificate issuance, backup restoration, Windows
  elevation policy, and multiple physical host/network combinations depend on
  the operator environment and must be validated with the release checklist.
- OpenH264 is software encoding. Performance and power use vary by machine.
- The video diagnostics overlay contains only counters, dimensions, codec, and
  latency. Frame bytes stay transient and are neither logged nor persisted.
- In-app updates require the configured updater signature. Authenticode is a
  separate optional publisher signature; release jobs require a signed tag and
  exact source/artifact version agreement before publication.
