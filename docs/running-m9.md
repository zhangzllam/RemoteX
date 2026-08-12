# Running RemoteX M9

Start PostgreSQL, the Control Server, and Relay as described in
`docs/running-m8.md`. M9 changes the Agent from automatic credential claim to an
explicit local authorization flow.

## Interactive access (default)

Do not set unattended-access variables. Start the Agent with the capabilities
the local user is willing to grant:

```powershell
$env:REMOTEX_CONTROL_URL = "http://127.0.0.1:8080"
$env:REMOTEX_IDENTITY_PATH = "$env:LOCALAPPDATA\RemoteX\identity.json"
$env:REMOTEX_RELAY_CA_CERT = "C:\certs\relay-cert.pem"
$env:REMOTEX_ALLOW_INPUT = "true"
$env:REMOTEX_ALLOW_CLIPBOARD = "true"
cargo run -p remotex-agent
```

After the Desktop requests a Session, Windows shows a foreground RemoteX dialog
with the Controller name and final capability list. Select **Yes** to accept or
**No** to reject. While connected, the Agent console shows a prominent active
Session banner. Press Ctrl+C there to disconnect immediately.

Capabilities that are disabled locally remain disabled even if requested by the
Controller. The Agent, not the Controller UI, enforces screen, input, clipboard,
upload, and download permission.

## Explicit unattended access

Unattended access requires an explicit switch and a secret of 12–128 bytes:

```powershell
$env:REMOTEX_UNATTENDED_ACCESS = "true"
$env:REMOTEX_UNATTENDED_SECRET = "use-a-long-random-secret"
cargo run -p remotex-agent
```

Enter the same value in the Desktop's optional unattended-secret field. A wrong
or missing value does not grant access; the Agent falls back to the local prompt.
The Control Server encrypts the pending secret at rest and never includes it in
logs or audit metadata.

For normal interactive access, leave the Desktop secret field empty. Do not put
production secrets in source control, shell history, screenshots, or shared
configuration files.

## Audit lifecycle

PostgreSQL records Session request, decision, start, and end events. Records
include IDs, Controller name, final permissions, connection type, timestamps,
result, and aggregate bytes. Remote desktop content, input, clipboard contents,
file paths/data, Relay tokens, E2EE keys, unattended secrets, and identity keys
are excluded.
