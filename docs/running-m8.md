# Running RemoteX M8

M8 replaces manually registered Relay Sessions with a PostgreSQL-backed Control
Server. The manual environment variables remain available only for focused local
testing.

## Prerequisites

- PostgreSQL with an empty `remotex` database
- a Relay QUIC certificate and private key as documented in `docs/relay.md`
- a random 32-byte Control Server master key encoded as 64 hexadecimal characters
- the Relay CA certificate available to the Agent and Desktop

Do not reuse example secrets in production and do not commit a `.env` file.

## Start the Control Server

```powershell
$env:REMOTEX_DATABASE_URL = "postgres://remotex:password@127.0.0.1/remotex"
$env:REMOTEX_CONTROL_MASTER_KEY_HEX = "<64 hex characters>"
$env:REMOTEX_CONTROL_BIND = "127.0.0.1:8080"
$env:REMOTEX_PUBLIC_RELAY_ADDRESS = "127.0.0.1:7443"
$env:REMOTEX_RELAY_SERVER_NAME = "localhost"
cargo run -p remotex-control
```

Migrations run automatically. `/health` and `/ready` expose the initial process
health surface; M13 expands readiness checks for production deployment.

## Start the Relay

```powershell
$env:REMOTEX_DATABASE_URL = "postgres://remotex:password@127.0.0.1/remotex"
$env:REMOTEX_RELAY_CERT = "C:\\certs\\relay-cert.pem"
$env:REMOTEX_RELAY_KEY = "C:\\certs\\relay-key.pem"
cargo run -p remotex-relay
```

The Relay consumes each role credential once in a database transaction. It does
not receive the Control Server master key or the E2EE Session key.

## Enroll and run the Windows Agent

```powershell
$env:REMOTEX_CONTROL_URL = "http://127.0.0.1:8080"
$env:REMOTEX_IDENTITY_PATH = "$env:LOCALAPPDATA\\RemoteX\\identity.json"
$env:REMOTEX_RELAY_CA_CERT = "C:\\certs\\relay-cert.pem"
$env:REMOTEX_DEVICE_NAME = "My Windows PC"
$env:REMOTEX_ALLOW_INPUT = "true"
cargo run -p remotex-agent
```

The Agent prints its Device ID, sends signed heartbeats, and polls for signed
Session claims. Its private identity key remains in the configured local file.
Clipboard and file permissions remain independently disabled unless their M6/M7
environment flags are enabled.

## Run the Desktop

```powershell
cargo run -p remotex-desktop
```

Enter the Control Server URL, nine-digit Device ID, Controller name, and Relay CA
certificate path. The Desktop requests its one-time credential directly; the
Agent independently claims its role credential. The manual fields in the
collapsible fallback section are not needed for a managed Session.

For production, put the HTTP Control API behind TLS. M13 provides the complete
self-hosting stack and reverse-proxy configuration.
