# Running the M3 Development Stack

M3 is a development-only relay video path. Device registration, session creation,
and authorization UI belong to M7/M8, so the relay currently receives one test
Session and two one-time credentials through environment variables.

## Prerequisites

- Windows 10/11 controller and agent
- Rust stable
- Node.js 22 and pnpm 11
- a TLS certificate whose subject covers the relay server name

Never commit private keys or session tokens. Use a locally trusted development
CA or a normally issued certificate. Both peers must trust the supplied CA file.
Generate a random 32-byte end-to-end key for each Session and provide the same
64-character hex value to the Agent and Controller. It never goes to the Relay.

## Build

```powershell
pnpm --dir apps/desktop/ui install --frozen-lockfile
pnpm --dir apps/desktop/ui build
cargo build --workspace
```

## Relay

Set a UUID Session ID, different 64-hex-character tokens for each role, the
certificate/key paths, and a bind address:

```powershell
$env:REMOTEX_RELAY_BIND = "0.0.0.0:7443"
$env:REMOTEX_RELAY_CERT = "C:\certs\relay-cert.pem"
$env:REMOTEX_RELAY_KEY = "C:\certs\relay-key.pem"
$env:REMOTEX_SESSION_ID = "00000000-0000-4000-8000-000000000001"
$env:REMOTEX_CONTROLLER_TOKEN_HEX = "<64 hex characters>"
$env:REMOTEX_AGENT_TOKEN_HEX = "<different 64 hex characters>"
$env:REMOTEX_TOKEN_LIFETIME_SECONDS = "300"
$env:RUST_LOG = "remotex_relay=info"
cargo run -p remotex-relay
```

## Agent

The agent chooses the primary monitor unless `REMOTEX_MONITOR_ID` is set to an
ID printed by the DXGI capture demo.

```powershell
$env:REMOTEX_RELAY_ADDRESS = "127.0.0.1:7443"
$env:REMOTEX_RELAY_SERVER_NAME = "localhost"
$env:REMOTEX_RELAY_CA_CERT = "C:\certs\relay-cert.pem"
$env:REMOTEX_SESSION_ID = "00000000-0000-4000-8000-000000000001"
$env:REMOTEX_AGENT_TOKEN_HEX = "<agent token>"
$env:REMOTEX_E2E_KEY_HEX = "<shared 64-hex-character session key>"
$env:REMOTEX_VIDEO_FPS = "12"
cargo run -p remotex-agent
```

## Controller

Start the Tauri development application:

```powershell
pnpm --dir apps/desktop/ui tauri dev
```

Enter the relay address, TLS server name, trusted CA path, Session ID,
controller token, and the same end-to-end session key, then select **Connect**.
Both secret fields are masked and secrets are never logged.

The standalone M2 capture check remains available:

```powershell
cargo run -p remotex-capture --example dxgi_capture_demo
```

It enumerates monitors, captures 100 frames, and saves
`remotex-dxgi-frame-100.png` in the current directory.
