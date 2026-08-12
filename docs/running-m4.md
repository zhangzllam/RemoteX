# Running the M4 Development Stack

M4 is a development-only relay video and mouse-control path. Device registration,
Session creation, and confirmation UI belong to M8/M9, so the Relay currently
receives one test Session and two one-time credentials through environment
variables. Input permission is configured explicitly on the Agent and is off by
default.

## Prerequisites

- Windows 10/11 Agent and Controller
- Rust stable
- Node.js 22 and pnpm 11
- a TLS certificate whose subject covers the Relay server name

Never commit private keys, Session tokens, or end-to-end keys. Generate a fresh
random 32-byte end-to-end key for each Session and supply the same 64-character
hex value only to the Agent and Controller.

## Build

```powershell
pnpm --dir apps/desktop/ui install --frozen-lockfile
pnpm --dir apps/desktop/ui build
cargo build --workspace --all-features
```

## Relay

```powershell
$env:REMOTEX_RELAY_BIND = "0.0.0.0:7443"
$env:REMOTEX_RELAY_CERT = "C:\certs\relay-cert.pem"
$env:REMOTEX_RELAY_KEY = "C:\certs\relay-key.pem"
$env:REMOTEX_SESSION_ID = "00000000-0000-4000-8000-000000000001"
$env:REMOTEX_CONTROLLER_TOKEN_HEX = "<64 hex characters>"
$env:REMOTEX_AGENT_TOKEN_HEX = "<different 64 hex characters>"
$env:REMOTEX_TOKEN_LIFETIME_SECONDS = "300"
$env:REMOTEX_RELAY_HEARTBEAT_INTERVAL_MS = "5000"
$env:REMOTEX_RELAY_PEER_TIMEOUT_MS = "30000"
$env:RUST_LOG = "remotex_relay=info"
cargo run -p remotex-relay
```

## Agent

The Agent chooses the primary monitor unless `REMOTEX_MONITOR_ID` is set to an
ID printed by the M2 capture demo. `REMOTEX_ALLOW_INPUT=true` is the explicit
local authorization for M4. Omit it or set it to false to verify that video
continues while all remote mouse events are denied before `SendInput`.

```powershell
$env:REMOTEX_RELAY_ADDRESS = "127.0.0.1:7443"
$env:REMOTEX_RELAY_SERVER_NAME = "localhost"
$env:REMOTEX_RELAY_CA_CERT = "C:\certs\relay-cert.pem"
$env:REMOTEX_SESSION_ID = "00000000-0000-4000-8000-000000000001"
$env:REMOTEX_AGENT_TOKEN_HEX = "<agent token>"
$env:REMOTEX_E2E_KEY_HEX = "<shared 64-hex-character session key>"
$env:REMOTEX_VIDEO_FPS = "12"
$env:REMOTEX_ALLOW_INPUT = "true"
$env:RUST_LOG = "remotex_agent=info"
cargo run -p remotex-agent
```

Keep this terminal visible. It reports `remote session active` after pairing and
never logs tokens, keys, or input payload contents.

## Controller and manual mouse test

```powershell
pnpm --dir apps/desktop/ui tauri dev
```

Enter the Relay address, TLS server name, trusted CA path, Session ID,
Controller token, and shared end-to-end key, then select **Connect**. Once the
remote image appears:

1. Move across all four edges and the center; verify the Agent cursor follows
   the displayed image and does not react over black letterbox bars.
2. Open a harmless context such as Notepad and verify left, right, and middle
   clicks.
3. Perform two normal left clicks and verify double-click behavior.
4. Scroll vertically and, where supported by the target application,
   horizontally.
5. Hold a mouse button and disconnect; verify no button remains pressed.
6. Restart the Agent with `REMOTEX_ALLOW_INPUT=false`; verify video still works
   but mouse input does not execute.

Windows may reject `SendInput` into applications running at a higher integrity
level than the Agent. Run the test target and Agent at the same integrity level;
M4 does not bypass Windows security boundaries.
