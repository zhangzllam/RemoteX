# RemoteX V1 release checklist

## Automated release gates

- [x] Rust formatting passes for the whole workspace.
- [x] Clippy passes for all targets/features with warnings denied.
- [x] Workspace unit, integration, and documentation tests pass.
- [x] Windows-specific Agent/desktop tests pass, including DPAPI.
- [x] Linux Agent tests and a real Linux target build pass.
- [x] Desktop TypeScript and production Vite build pass.
- [x] Malformed, oversized, truncated, and corrupted protocol input is rejected.
- [x] Replay, authorization, permission intersection, queue limits, file-root
  traversal, resume/cancel, checksum, input release, Relay cleanup, and PTY
  cleanup have automated coverage.
- [x] RustSec audit passes with the documented unreachable SQLx-MySQL exception.
- [x] Production frontend audit reports zero known vulnerabilities.
- [x] Windows NSIS packaging produces an installer and SHA-256 file.
- [x] Tagged release automation packages Windows and Linux artifacts.

## Operator validation before exposing a deployment

- [ ] Point real Control and Relay DNS names at the production host.
- [ ] Install a CA-issued Relay certificate and verify renewal procedures.
- [ ] Create unique deployment secrets with restrictive file permissions.
- [ ] Verify Caddy HTTPS, Control `/health` and `/ready`, Relay UDP reachability,
  PostgreSQL isolation, and external rate limiting/network policy.
- [ ] Complete one interactive Windows session across the intended networks;
  verify view, granted input/clipboard/files, rejection, and local disconnect.
- [ ] Complete one authorized Linux PTY/files/system-info session and confirm the
  service is enabled only if unattended startup is intended.
- [ ] Exercise Relay fallback when a direct candidate is unreachable.
- [ ] Restore a database backup in a separate environment.
- [ ] Verify release SHA-256 values and, when configured, Authenticode signature.
- [ ] Confirm logs/backups contain no credentials or session content and set a
  retention policy for database audit/session records.

Unchecked items are environment-specific acceptance tests; they cannot be
truthfully completed by a single-machine source build.
