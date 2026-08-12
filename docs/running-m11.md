# Running M11 direct connectivity

M11 keeps Relay as the authorization rendezvous and reliable fallback. Once the
authorized Agent and Controller both receive `PeerReady`, they spend at most
three seconds upgrading the data path to a direct QUIC connection. If discovery,
TLS, UDP reachability, or direct peer authentication fails, the already-open
Relay path is used immediately.

## Agent configuration

Direct connectivity is opt-in because the Agent needs a TLS private key and a
fixed UDP listening port:

```powershell
$env:REMOTEX_DIRECT_CERT = "C:\RemoteX\direct-chain.pem"
$env:REMOTEX_DIRECT_KEY = "C:\RemoteX\direct-key.pem"
$env:REMOTEX_DIRECT_SERVER_NAME = "direct.remote.example.com"
$env:REMOTEX_DIRECT_BIND = "0.0.0.0:7444"
```

The direct certificate must chain to the CA configured in the Controller's
`caCertificatePath`. The Agent automatically advertises its primary LAN address.
Override it when routing discovery is unsuitable:

```powershell
$env:REMOTEX_DIRECT_LAN_ADDRESS = "192.168.1.20:7444"
```

For an Internet-reachable UDP mapping or explicit port forward, advertise its
server-reflexive address as well:

```powershell
$env:REMOTEX_DIRECT_PUBLIC_ADDRESS = "203.0.113.20:7444"
```

Open only UDP 7444 (or the configured port). No TCP shell or unauthenticated
service is exposed. Symmetric NATs and networks that block peer UDP fall back to
Relay; RemoteX does not guess mappings or require a public STUN dependency.

## Selection and authentication

Signed Agent heartbeats publish at most 16 validated candidates through the
Control Server. LAN candidates sort before server-reflexive candidates. The
Controller shows `Connection: LAN`, `Direct`, or `Relay` in its status text, and
the selected type is stored in the Session audit record.

TLS authenticates the configured direct server name. The direct stream then
performs a mutual challenge using fresh 256-bit client/server nonces and
HMAC-SHA256 proofs derived from the short-lived E2EE Session key. Proofs bind the
RemoteX direct-auth domain, role, Session ID, and both nonces. A reachable UDP
port alone never grants a Session. Payloads keep the same direction-separated
XChaCha20-Poly1305 envelopes used through Relay.

