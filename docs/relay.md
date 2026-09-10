# M1 Relay Development Guide

M1 provides a development-only Controller ↔ Relay ↔ Agent path. It uses Quinn
with normal TLS certificate validation, an in-memory Session authenticator, one
bidirectional stream per peer, explicit length framing, and bincode protocol
messages. It does not provide production PKI or the M7 Control Server.

## Build and development certificate

Generate a local certificate for `localhost` into a directory outside the
repository:

```powershell
cargo run -p remotex-relay --example generate_dev_certificate -- C:\remotex-dev-certs
```

The Controller and Agent trust `relay-cert.pem`; only the Relay reads
`relay-key.pem`. This certificate is development-only. Never commit, share, or
reuse its private key in production.

Generate a Session ID and two different 256-bit tokens:

```powershell
$sessionId = [guid]::NewGuid().ToString()
function New-RemoteXToken {
  $bytes = New-Object byte[] 32
  $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
  try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }
  [BitConverter]::ToString($bytes).Replace("-", "").ToLower()
}
$controllerToken = New-RemoteXToken
$agentToken = New-RemoteXToken
```

Use those same three values in all terminals below. Do not paste real secrets
into logs, issues, or commits.

## Terminal 1 — Relay

```powershell
$env:REMOTEX_RELAY_BIND = "127.0.0.1:7443"
$env:REMOTEX_RELAY_CERT = "C:\remotex-dev-certs\relay-cert.pem"
$env:REMOTEX_RELAY_KEY = "C:\remotex-dev-certs\relay-key.pem"
$env:REMOTEX_SESSION_ID = $sessionId
$env:REMOTEX_CONTROLLER_TOKEN_HEX = $controllerToken
$env:REMOTEX_AGENT_TOKEN_HEX = $agentToken
$env:REMOTEX_TOKEN_LIFETIME_SECONDS = "300"
$env:RUST_LOG = "remotex_relay=info"
cargo run -p remotex-relay
```

## Terminal 2 — Mock Agent

```powershell
$env:REMOTEX_RELAY_ADDRESS = "127.0.0.1:7443"
$env:REMOTEX_RELAY_SERVER_NAME = "localhost"
$env:REMOTEX_RELAY_CA_CERT = "C:\remotex-dev-certs\relay-cert.pem"
$env:REMOTEX_SESSION_ID = $sessionId
$env:REMOTEX_AGENT_TOKEN_HEX = $agentToken
$env:RUST_LOG = "info"
cargo run -p remotex-relay --example mock_agent
```

The Agent authenticates and logs that it is waiting for the Controller.

## Terminal 3 — Mock Controller

```powershell
$env:REMOTEX_RELAY_ADDRESS = "127.0.0.1:7443"
$env:REMOTEX_RELAY_SERVER_NAME = "localhost"
$env:REMOTEX_RELAY_CA_CERT = "C:\remotex-dev-certs\relay-cert.pem"
$env:REMOTEX_SESSION_ID = $sessionId
$env:REMOTEX_CONTROLLER_TOKEN_HEX = $controllerToken
$env:RUST_LOG = "info"
cargo run -p remotex-relay --example mock_controller
```

Both peers receive `PeerReady`. The Controller sends `ControlMessage::Ping`,
the Agent logs receipt and returns `ControlMessage::Pong`, and the Controller
logs the matching response before both clients close cleanly.

## Configuration

| Variable | Default | Purpose |
| --- | ---: | --- |
| `REMOTEX_RELAY_BIND` | required | Relay UDP listen address |
| `REMOTEX_RELAY_CERT` | required | PEM certificate chain |
| `REMOTEX_RELAY_KEY` | required | PEM private key |
| `REMOTEX_TOKEN_LIFETIME_SECONDS` | `300` | Test credential lifetime |
| `REMOTEX_RELAY_MAX_MESSAGE_SIZE` | `8388608` | Maximum framed Relay message bytes |
| `REMOTEX_RELAY_MAX_CONNECTIONS` | `1024` | Concurrent authenticated/handshaking connections |
| `REMOTEX_RELAY_MAX_PENDING_SESSIONS` | `512` | Session registry entries |
| `REMOTEX_RELAY_QUEUE_CAPACITY` | `32` | Per-peer bounded outbound messages |
| `REMOTEX_RELAY_HANDSHAKE_TIMEOUT_MS` | `10000` | Stream/ClientHello deadline |
| `REMOTEX_RELAY_HEARTBEAT_INTERVAL_MS` | `5000` | Relay heartbeat interval |
| `REMOTEX_RELAY_PEER_TIMEOUT_MS` | `30000` | Maximum inbound inactivity |

The 8 MiB default retains compatibility with M3 JPEG payloads. M1-only control
deployments should lower `REMOTEX_RELAY_MAX_MESSAGE_SIZE` to a conservative
value such as `262144` bytes.

## Automated validation

```powershell
cargo test -p remotex-relay --all-features
```

The integration suite covers successful pairing, each forwarding direction,
invalid token, role mismatch, duplicate role, expired Session, oversized frame,
peer disconnect cleanup, heartbeat timeout cleanup, and the M3 encrypted-video
opaque payload path.
